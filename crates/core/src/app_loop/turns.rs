//! Turns: what the loop does with something a speaker said, with a turn that ends, and with the reply.

use super::App;
use crate::session::TurnCtl;
use crate::turns::{self, Event as HubEvent, Origin, Reply, Request, Submitted};

pub(crate) const NO_LLM: &str =
    "(that needs a language model; start with --llm-url and --llm-model)";

pub(crate) const TOO_BUSY: &str = "I'm still working on your last requests. Ask again in a moment.";

/// Show a reply: to the terminal for the console, or back to the satellite that asked.
fn deliver(origin: Origin, reply: Reply) {
    if let Some(reply) = origin.deliver(reply) {
        if reply.say.is_empty() {
            println!("speaker> (done)");
        } else {
            println!("speaker> {}", reply.say);
        }
        for e in &reply.effects {
            println!("satellite> {e:?}");
        }
    }
}

/// If the timer, alarm or reminder the reply is about could not be saved, say so in the reply itself: it is set
/// and will ring, but a restart would lose it, and the person should know before they rely on it.
fn tell_if_unsaved(reply: &mut Reply, outcome: Result<(), crate::app::persist::SaveError>) {
    if outcome.is_err() {
        reply.say = crate::assistant::say_not_saved(&reply.say);
    }
}

impl App {
    /// Handle something a speaker said: the fast path answers at once, anything else becomes a turn.
    pub(super) fn submit(&mut self, req: Request) {
        if let Some((origin, reply)) = self.dispatch(req) {
            self.deliver_confirmed(origin, reply);
        }
    }

    /// Handle something a speaker said: the fast path answers at once; anything else becomes a turn beside the
    /// loop (or waits for the same speaker's running turn). Returns the reply when there is one already.
    pub(crate) fn dispatch(&mut self, req: Request) -> Option<(Origin, Reply)> {
        if let Some(reply) = self.brain.fast(&req.line) {
            return Some((req.origin, reply));
        }
        // Only the handle on the model is copied here; the kit (tool specs, system prompt) is built by the hub
        // when a task starts, not for a request that is queued or refused.
        let Some(llm) = self.brain.llm.clone() else {
            return Some((
                req.origin,
                Reply {
                    say: NO_LLM.into(),
                    ..Reply::default()
                },
            ));
        };
        let now = self.brain.mono();
        let brain = &mut self.brain;
        match self.hub.submit(req, now, || brain.kit(&llm)) {
            Submitted::Started | Submitted::Queued => None,
            Submitted::Refused(req) => Some((
                req.origin,
                Reply {
                    say: TOO_BUSY.into(),
                    ..Reply::default()
                },
            )),
        }
    }

    /// A turn task, tool call or realtime session reported.
    pub async fn on_hub_event(&mut self, ev: HubEvent) {
        match ev {
            HubEvent::Tool(req) => req.serve(&mut self.brain.assistant).await,
            HubEvent::Turn(f) => {
                let (origin, reply) = self.finish_turn(*f);
                self.deliver_confirmed(origin, reply);
            }
            HubEvent::Other(_) => {
                // A realtime session ended: its audio cost is in the counters; show how it went.
                self.brain.save_usage();
                self.core
                    .patch_ui_status("realtime", self.brain.rt.status());
            }
            HubEvent::Lost(speaker) => {
                // The satellite waiting for this answer sees its channel close and says it cannot answer.
                crate::diag::error("a turn ended without an answer");
                let now = self.brain.mono();
                if let Some(next) = self.hub.lost(&speaker, now)
                    && let Some((origin, reply)) = self.dispatch(next)
                {
                    self.deliver_confirmed(origin, reply);
                }
            }
        }
    }

    /// A turn is over: put the speaker's conversation back, save the counters, start what was waiting for the
    /// same speaker, and return the reply to deliver.
    fn finish_turn(&mut self, f: turns::Finished) -> (Origin, Reply) {
        self.brain.save_usage();
        if let Some(e) = &f.failed {
            crate::diag::error(&format!("language model error: {e}"));
        }
        let now = self.brain.mono();
        let (origin, reply, next) = self.hub.finish(f, now);
        if let Some(next) = next
            && let Some((o, r)) = self.dispatch(next)
        {
            self.deliver_confirmed(o, r);
        }
        (origin, reply)
    }

    /// Let the turns that are running finish and show their replies (the console was closed).
    pub(super) async fn drain(&mut self) {
        while self.hub.active() > 0 {
            let ev = self.hub.next().await;
            self.on_hub_event(ev).await;
        }
        // Replies held back for a write are delivered by their own tasks; let them run before the caller leaves.
        if let Some(w) = self.core.writer() {
            let _ = w.flushed_since(0, &|_| true).await;
        }
        tokio::task::yield_now().await;
    }

    /// Show a reply once any timer or alarm change it may announce is on disk. A crash right after "Timer set"
    /// is spoken must not lose the timer, so the reply is held back for the write (a few milliseconds on a
    /// laptop, tens to hundreds on slow flash). The wait happens in its own task, so the loop keeps running.
    pub(crate) fn deliver_confirmed(&self, origin: Origin, mut reply: Reply) {
        match self.brain.assistant.take_save() {
            None => deliver(origin, reply),
            Some(saved) => match saved.outcome_now() {
                Some(outcome) => {
                    tell_if_unsaved(&mut reply, outcome);
                    deliver(origin, reply);
                }
                None => {
                    tokio::spawn(async move {
                        let outcome = saved.wait().await;
                        tell_if_unsaved(&mut reply, outcome);
                        deliver(origin, reply);
                    });
                }
            },
        }
    }

    /// A reply held back for the write of the change it announces (see [`App::deliver_confirmed`]), for the
    /// caller that returns the reply instead of delivering it.
    async fn confirmed(&self, mut reply: Reply) -> Reply {
        if let Some(saved) = self.brain.assistant.take_save() {
            tell_if_unsaved(&mut reply, saved.wait().await);
        }
        reply
    }

    /// Answer one thing and wait for it, serving the turn's tool calls meanwhile. For one-shot use instead of
    /// the loop (a recorded utterance).
    pub async fn respond_inline(&mut self, line: &str) -> Reply {
        let req = Request {
            speaker: "console".into(),
            line: line.to_owned(),
            ctl: TurnCtl::default(),
            origin: Origin::Console,
        };
        if let Some((_, reply)) = self.dispatch(req) {
            return self.confirmed(reply).await;
        }
        loop {
            match self.hub.next().await {
                HubEvent::Tool(req) => req.serve(&mut self.brain.assistant).await,
                HubEvent::Turn(f) => {
                    let reply = self.finish_turn(*f).1;
                    return self.confirmed(reply).await;
                }
                HubEvent::Lost(_) => {
                    return Reply {
                        say: "Sorry, I can't answer right now.".into(),
                        ..Reply::default()
                    };
                }
                HubEvent::Other(_) => {}
            }
        }
    }
}
