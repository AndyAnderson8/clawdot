//! The running application: the one loop that owns the [`Brain`] and the conversation [`Hub`] (ADR-0021).
//!
//! [`App`] holds what the loop owns (the brain, the hub, the settings in effect, the [`Sources`] it listens on,
//! the tick and the handle on the speaker task) and has one method per kind of event; all of the loop's
//! behaviour is on it, split by concern: `turns` (a request becomes a turn, a turn ends, a reply is delivered),
//! `portal` (changes asked for in the web UI) and `connections` (MCP connections made beside the loop).
//! [`App::run`] only chooses which event is next. Turns, realtime sessions, speech and MCP connections are
//! tasks beside the loop; they report back through the sources, and nothing here waits for them. The start-up
//! sequence that builds an `App` is in `startup`; `--voice-in` builds one with [`App::one_shot`] and drives the
//! same methods without running the loop.
//!
//! Names. "Speaker" means three things in this crate and each has its own words: `speakers` is the set of
//! per-speaker conversations (the [`Hub`]'s keys), `speaker` is the task that says things in order, and
//! [`SpeakerHandle`] here is this loop's end of that task. "Inbox" always means the agent inbox (`crate::inbox`);
//! the channels this loop listens on are [`Sources`].

use crate::app::{Core, Job};
use crate::assistant::describe_fired;
use crate::connects::Done;
use crate::rtvoice;
use crate::runtime::{Brain, SpeakerView};
use crate::satellite::{Announcer, Ask};
use crate::session::TurnCtl;
use crate::speaker::{Say, Voice};
use crate::turns::{Hub, Origin, Request};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Interval;

mod connections;
mod portal;
mod turns;

/// What the loop does after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// The application is done: the counters and stores are saved.
    Exit,
}

/// The channels the loop listens on. Each is the receiving end of a task or front end that reports back.
#[derive(Debug)]
pub struct Sources {
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

impl Sources {
    /// Only the finished connections, for the one-shot command: every other source is closed.
    #[must_use]
    pub fn mcp_only(mcp: mpsc::Receiver<Done>) -> Self {
        Self {
            mcp,
            ..Self::closed()
        }
    }

    fn closed() -> Self {
        Self {
            portal: mpsc::channel(1).1,
            mcp: mpsc::channel(1).1,
            realtime: mpsc::channel(1).1,
            asks: mpsc::channel(1).1,
            lines: mpsc::channel(1).1,
            console: false,
        }
    }
}

/// The loop's end of the speaker task: everything that speaks is handed over and said in order, so the wait
/// for a voice or a summary never holds the loop.
#[derive(Debug)]
pub struct SpeakerHandle {
    pub tx: mpsc::UnboundedSender<Say>,
    pub voice: Arc<Voice>,
    pub announcer: Announcer,
    /// Speech is printed to the terminal, which counts as a speaker.
    pub console: bool,
    /// Set once the speaker task is gone, so that the loss is reported once, not every second.
    pub lost: bool,
}

impl SpeakerHandle {
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
    pub(crate) sources: Sources,
    /// The speaker task, or `None` for the one-shot command, which has nothing to speak and no tick.
    pub(crate) speaker_task: Option<SpeakerHandle>,
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
        sources: Sources,
        speaker_task: SpeakerHandle,
    ) -> Self {
        let mut app = Self::one_shot(brain, hub, core, applied, sources);
        app.speaker_task = Some(speaker_task);
        app
    }

    /// An application that is not run as a loop: the one-shot command answers a recorded utterance by
    /// calling the event methods itself. It has no speaker task, so it has no tick.
    pub fn one_shot(
        brain: Brain,
        hub: Hub,
        core: Arc<Core>,
        applied: serde_json::Map<String, serde_json::Value>,
        sources: Sources,
    ) -> Self {
        Self {
            brain,
            hub,
            core,
            applied,
            sources,
            speaker_task: None,
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
                Some(job) = self.sources.portal.recv() => {
                    self.on_portal_job(job).await;
                    Flow::Continue
                }
                Some(done) = self.sources.mcp.recv() => {
                    self.on_mcp_connected(done);
                    Flow::Continue
                }
                line = self.sources.lines.recv(), if self.sources.console => {
                    self.on_console_line(line).await
                }
                Some(job) = self.sources.realtime.recv() => {
                    self.on_realtime_job(job);
                    Flow::Continue
                }
                Some(ask) = self.sources.asks.recv() => {
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

    /// A line from the terminal. `None` means the terminal was closed: the turns that are running finish and
    /// show their replies first. "quit" leaves at once.
    pub async fn on_console_line(&mut self, line: Option<String>) -> Flow {
        let Some(line) = line else {
            self.drain().await;
            self.save_all();
            return Flow::Exit;
        };
        if line.is_empty() {
            return Flow::Continue;
        }
        if crate::commands::is_quit(self.brain.assistant.lang(), &line) {
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

    /// A speaker asked for a realtime voice session.
    pub fn on_realtime_job(&mut self, job: rtvoice::Job) {
        self.brain.start_realtime(job, &self.core, &mut self.hub);
    }

    /// Once a second: sweep stale conversations, publish the status, hand what is due to the speaker task.
    /// Without a speaker task (the one-shot command) there is no tick, and this does nothing.
    pub fn on_tick(&mut self) {
        let Some(speaker) = self.speaker_task.as_mut() else {
            return;
        };
        self.hub.sweep(self.brain.mono());
        publish_status(&self.core, &self.brain, speaker, &self.stations);
        // Everything that speaks is handed to the speaker task (ADR-0021).
        while let Some((text, preview)) = self.core.take_speech_as() {
            speaker.say(Say::Phrase { text, preview });
        }
        for fired in self.brain.tick() {
            speaker.say(Say::Line(describe_fired(&fired)));
        }
        if !speaker.voice.polling.swap(true, Ordering::Relaxed) {
            let poll = Say::Poll {
                minute: self.brain.minute_of_day(),
                day: self.brain.local_day(),
                llm: self.brain.model(),
            };
            speaker.say(poll);
        }
        self.core.maintain();
    }

    fn save_all(&self) {
        self.brain.save_usage();
        self.core.flush_blocking();
    }
}

fn publish_status(core: &Core, brain: &Brain, speaker: &SpeakerHandle, stations: &[String]) {
    let connected = speaker.announcer.connected();
    let view = SpeakerView {
        available: connected > 0 && speaker.voice.available.load(Ordering::Relaxed),
        console: speaker.console,
        quiet: core.quiet_hours(),
    };
    core.set_ui_status(brain.status_json(connected, &view, stations, &brain.tool_counts()));
}

#[cfg(test)]
mod tests;
