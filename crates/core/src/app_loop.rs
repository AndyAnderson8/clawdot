//! The running application: the one loop that owns the [`Brain`] and the conversation [`Hub`] (ADR-0021).
//!
//! [`App`] holds what the loop owns (the brain, the hub, the settings in effect, the receivers it listens
//! on, the tick and the handle on the speaker task) and has one small method per kind of event. [`App::run`]
//! only chooses which event is next. Turns, realtime sessions, speech and MCP connections are tasks beside
//! the loop; they report back through the receivers, and nothing here waits for them. The start-up sequence
//! that builds an `App` is in `startup`.

use crate::app::{Core, Job};
use crate::assistant::describe_fired;
use crate::connects::Done;
use crate::rtvoice;
use crate::runtime::{
    Brain, SpeakerView, deliver_confirmed, dispatch, drain, mcp_connected, on_hub_event, portal_op,
};
use crate::satellite::{Announcer, Ask};
use crate::session::TurnCtl;
use crate::speaker::{Say, Voice};
use crate::turns::{Event as HubEvent, Hub, Origin, Request};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Interval;

/// What the loop does after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// The application is done: the counters and stores are saved.
    Exit,
}

/// The channels the loop listens on. Each is the receiving end of a task or front end that reports back.
#[derive(Debug)]
pub struct Inbox {
    /// Changes asked for in the web UI.
    pub portal: mpsc::Receiver<Job>,
    /// Finished MCP connections.
    pub mcp: mpsc::Receiver<Done>,
    /// Requests to start a realtime voice session.
    pub realtime: mpsc::Receiver<rtvoice::Job>,
    /// Questions from satellites.
    pub asks: mpsc::Receiver<Ask>,
    /// Lines typed at the terminal; only listened to when `console` is set.
    pub lines: mpsc::Receiver<String>,
    pub console: bool,
}

/// The loop's side of the speaker task: everything that speaks is handed over and said in order, so the wait
/// for a voice or a summary never holds the loop.
#[derive(Debug)]
pub struct Speaker {
    pub tx: mpsc::UnboundedSender<Say>,
    pub voice: Arc<Voice>,
    pub announcer: Announcer,
    /// Speech is printed to the terminal, which counts as a speaker.
    pub console: bool,
    /// Set once the speaker task is gone, so that the loss is reported once, not every second.
    pub lost: bool,
}

impl Speaker {
    fn say(&mut self, item: Say) {
        if self.tx.send(item).is_err() && !self.lost {
            self.lost = true;
            crate::diag::error(
                "the speaker task has stopped, so nothing more will be spoken; restart ClawDot",
            );
        }
    }
}

/// The running application. See the module documentation.
#[derive(Debug)]
pub struct App {
    pub(crate) brain: Brain,
    pub(crate) hub: Hub,
    pub(crate) core: Arc<Core>,
    /// The settings in effect, so a change does work only where something differs.
    pub(crate) applied: serde_json::Map<String, serde_json::Value>,
    pub(crate) inbox: Inbox,
    pub(crate) speaker: Speaker,
    /// Names of the music stations, for the status page.
    pub(crate) stations: Vec<String>,
    tick: Interval,
}

impl App {
    /// An application over a started [`Brain`]: once a second it sweeps, publishes the status and hands
    /// anything due to the speaker. Must be called inside the runtime.
    pub fn new(
        brain: Brain,
        hub: Hub,
        core: Arc<Core>,
        applied: serde_json::Map<String, serde_json::Value>,
        inbox: Inbox,
        speaker: Speaker,
    ) -> Self {
        Self {
            brain,
            hub,
            core,
            applied,
            inbox,
            speaker,
            stations: Vec::new(),
            tick: tokio::time::interval(Duration::from_secs(1)),
        }
    }

    /// The music stations the status page names.
    #[must_use]
    pub fn with_stations(mut self, stations: Vec<String>) -> Self {
        self.stations = stations;
        self
    }

    /// Serve events until the terminal asks to leave.
    pub async fn run(mut self) -> ExitCode {
        loop {
            let flow = tokio::select! {
                Some(job) = self.inbox.portal.recv() => {
                    self.on_portal_job(job).await;
                    Flow::Continue
                }
                Some(done) = self.inbox.mcp.recv() => {
                    self.on_mcp_connected(done);
                    Flow::Continue
                }
                line = self.inbox.lines.recv(), if self.inbox.console => {
                    self.on_console_line(line).await
                }
                Some(job) = self.inbox.realtime.recv() => {
                    self.on_realtime_job(job);
                    Flow::Continue
                }
                Some(ask) = self.inbox.asks.recv() => {
                    self.on_ask(ask);
                    Flow::Continue
                }
                ev = self.hub.next() => {
                    self.on_hub_event(ev).await;
                    Flow::Continue
                }
                _ = self.tick.tick() => {
                    self.on_tick();
                    Flow::Continue
                }
            };
            if flow == Flow::Exit {
                return ExitCode::SUCCESS;
            }
        }
    }

    /// A change the web UI asked for.
    pub async fn on_portal_job(&mut self, job: Job) {
        portal_op(&mut self.brain, &self.core, &mut self.applied, job).await;
    }

    /// A connection task reported.
    pub fn on_mcp_connected(&mut self, done: Done) {
        mcp_connected(&mut self.brain, &self.core, done);
    }

    /// A line from the terminal. `None` means the terminal was closed: the turns that are running finish and
    /// show their replies first. "quit" leaves at once.
    pub async fn on_console_line(&mut self, line: Option<String>) -> Flow {
        let Some(line) = line else {
            drain(&mut self.brain, &mut self.hub, &self.core).await;
            self.save_all();
            return Flow::Exit;
        };
        if line.is_empty() {
            return Flow::Continue;
        }
        if crate::commands::is_quit(&line) {
            self.save_all();
            return Flow::Exit;
        }
        self.submit(Request {
            speaker: "console".into(),
            line,
            ctl: TurnCtl::default(),
            origin: Origin::Console,
        });
        Flow::Continue
    }

    /// A question from a satellite.
    pub fn on_ask(&mut self, ask: Ask) {
        let ctl = TurnCtl {
            cancel: ask.cancel.clone(),
            speak: ask.progress.clone(),
            clock: None,
        };
        self.submit(Request {
            speaker: ask.speaker,
            line: ask.text,
            ctl,
            origin: Origin::Ask(ask.reply),
        });
    }

    /// A turn task, tool call or realtime session reported.
    pub async fn on_hub_event(&mut self, ev: HubEvent) {
        on_hub_event(&mut self.brain, &mut self.hub, &self.core, ev).await;
    }

    /// A speaker asked for a realtime voice session.
    pub fn on_realtime_job(&mut self, job: rtvoice::Job) {
        self.brain.start_realtime(job, &self.core, &mut self.hub);
    }

    /// Once a second: sweep stale conversations, publish the status, hand what is due to the speaker.
    pub fn on_tick(&mut self) {
        self.hub.sweep(self.brain.mono());
        self.publish_status();
        // Everything that speaks is handed to the speaker task (ADR-0021).
        while let Some((text, preview)) = self.core.take_speech_as() {
            self.speaker.say(Say::Phrase { text, preview });
        }
        for fired in self.brain.tick() {
            self.speaker.say(Say::Line(describe_fired(&fired)));
        }
        if !self.speaker.voice.polling.swap(true, Ordering::Relaxed) {
            let poll = Say::Poll {
                minute: self.brain.minute_of_day(),
                day: self.brain.local_day(),
                llm: self.brain.model(),
            };
            self.speaker.say(poll);
        }
        self.core.maintain();
    }

    /// Handle something a speaker said: the fast path answers at once, anything else becomes a turn.
    fn submit(&mut self, req: Request) {
        if let Some((origin, reply)) = dispatch(&mut self.brain, &mut self.hub, req) {
            deliver_confirmed(&self.brain, origin, reply);
        }
    }

    fn publish_status(&self) {
        let connected = self.speaker.announcer.connected();
        let view = SpeakerView {
            available: connected > 0 && self.speaker.voice.available.load(Ordering::Relaxed),
            console: self.speaker.console,
            quiet: self.core.quiet_hours(),
        };
        self.core.set_ui_status(self.brain.status_json(
            connected,
            &view,
            &self.stations,
            &self.brain.tool_counts(),
        ));
    }

    fn save_all(&self) {
        self.brain.save_usage();
        self.core.flush_blocking();
    }
}

#[cfg(test)]
mod tests;
