//! The speaker task: everything that talks (portal phrases, timer lines, agent-event announcements) is handed
//! to it and said in order, so waiting for a voice or a summary never holds the main loop (ADR-0021).

use crate::app::Core;
use crate::assistant::describe_fired;
use crate::inbox::event::Event;
use crate::inbox::summary::{
    SummaryConfig, announcement, clamp_for_speech, fallback_summary, short_announcement,
    summary_prompt,
};
use crate::llm::{ChatRequest, Message};
use crate::runtime::Llm;
use crate::satellite;
use crate::sched::{Fired, MISSED_GRACE_SECS};
use crate::session::ChatBackend;
use crate::store::JsonStore;
use crate::usage::UsageTracker;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

/// Something for the speaker task to say, handled in the order it was sent (ADR-0021).
#[derive(Debug)]
pub enum Say {
    /// A phrase from the portal; a voice being tried is used for this phrase only.
    Phrase {
        text: String,
        preview: Option<String>,
    },
    /// A timer, alarm or reminder that rang. The item rather than its wording is carried, because a line that
    /// had to wait for a satellite is worded as missed once it is too late to ring (REQ-TMR-015).
    Line(Fired),
    /// Look for an agent event worth announcing now, and say it.
    Poll {
        minute: u16,
        day: i64,
        llm: Option<Llm>,
    },
}

/// What the speaker task needs.
pub struct Voice {
    pub announcer: satellite::Announcer,
    pub core: Arc<Core>,
    pub usage: UsageTracker,
    pub usage_store: JsonStore,
    /// Spoken text is echoed only to an interactive console: in server mode stdout goes to a journal or
    /// container log that is never size-capped (REQ-SEC-009, PRIVACY.md).
    pub echo: bool,
    /// Agent events are for a satellite to speak: one is configured and nobody is reading a terminal. While
    /// none is connected (a speaker that is off, or its network link down) an event waits in the queue; it is
    /// not summarised and marked as announced with nobody to hear it.
    pub waits_for_satellite: bool,
    /// Some satellite could speak right now, as of the last poll (read by the loop for the portal).
    pub available: AtomicBool,
    /// A poll is queued or running; the loop sends another only when this is false.
    pub polling: AtomicBool,
}

/// Whether an agent event may be announced now. A connected satellite must be free (not busy or muted). With
/// none connected, the event is shown at the terminal or in the portal only when no satellite is meant to
/// speak it.
fn may_announce(connected: bool, free: bool, waits_for_satellite: bool) -> bool {
    if connected {
        free
    } else {
        !waits_for_satellite
    }
}

/// How many rung lines are kept for a satellite that is not there. Older ones are dropped first.
const MAX_WAITING_LINES: usize = 16;

/// A rung line kept until a satellite can speak it.
#[derive(Debug)]
struct Waiting {
    fired: Fired,
    /// [`Core::monotonic_secs`] when it was handed over, so the wait is measured on a clock that the network
    /// time cannot step.
    since: i64,
}

/// Keep a rung line for later, within [`MAX_WAITING_LINES`].
fn keep_line(waiting: &mut VecDeque<Waiting>, fired: Fired, now_mono: i64) {
    while waiting.len() >= MAX_WAITING_LINES {
        waiting.pop_front();
    }
    waiting.push_back(Waiting {
        fired,
        since: now_mono,
    });
}

/// The line for something that waited: the wait counts as lateness, and past [`MISSED_GRACE_SECS`] it is said
/// as missed rather than as ringing now.
fn line_after_wait(w: &Waiting, now_mono: i64) -> String {
    let waited = now_mono.saturating_sub(w.since).max(0);
    let late_secs = w.fired.late_secs.saturating_add(waited);
    describe_fired(&Fired {
        late_secs,
        missed: w.fired.missed || late_secs > MISSED_GRACE_SECS,
        ..w.fired.clone()
    })
}

/// Speak the waiting lines, oldest first, while a satellite takes them. The first one that nobody could take
/// (none connected, or busy or muted) ends the attempt and stays kept.
async fn speak_waiting(v: &Voice, waiting: &mut VecDeque<Waiting>) {
    while v.announcer.connected() > 0 {
        let Some(front) = waiting.front() else { return };
        let line = line_after_wait(front, v.core.monotonic_secs());
        if !v.announcer.announce(&line).await {
            return;
        }
        waiting.pop_front();
    }
}

/// Say what the loop hands over, one thing at a time. A timer's line waits for a satellite when one is meant to
/// speak it, as an agent event does; otherwise (none configured, or a terminal echoing speech) a satellite that
/// is busy or muted simply does not speak it (best effort).
pub async fn speak(voice: Arc<Voice>, mut rx: mpsc::UnboundedReceiver<Say>) {
    let v = &*voice;
    let mut waiting: VecDeque<Waiting> = VecDeque::new();
    while let Some(job) = rx.recv().await {
        match job {
            Say::Phrase { text, preview } => {
                if v.echo {
                    println!("speaker> {text}");
                }
                if v.announcer.connected() > 0 {
                    // The preview voice travels with the request; the configured voice is never touched.
                    v.announcer
                        .announce_in_voice(&text, preview.as_deref())
                        .await;
                }
            }
            Say::Line(fired) => {
                if v.echo {
                    println!("speaker> {}", describe_fired(&fired));
                }
                if v.waits_for_satellite {
                    // Behind any line already waiting, so they are said in the order they rang. Nothing here
                    // waits for a satellite to appear: the next poll tries again.
                    keep_line(&mut waiting, fired, v.core.monotonic_secs());
                    speak_waiting(v, &mut waiting).await;
                } else if v.announcer.connected() > 0 {
                    v.announcer.announce(&describe_fired(&fired)).await;
                }
            }
            Say::Poll { minute, day, llm } => {
                speak_waiting(v, &mut waiting).await;
                let connected = v.announcer.connected() > 0;
                let free = connected && v.announcer.is_available().await;
                v.available.store(free, Ordering::Relaxed);
                // A satellite that is busy, muted or (when one is meant to speak) not connected: keep the event
                // queued and try again.
                if may_announce(connected, free, v.waits_for_satellite)
                    && let Some(next) = v.core.next_announcement(true, minute)
                {
                    let spoken = match &llm {
                        Some(l) => {
                            let mut usage = v.usage.clone();
                            let s = summarise(l, &mut usage, day, &next.event).await;
                            usage.save_queued(&v.usage_store, v.core.writer().as_deref());
                            s
                        }
                        // No model: say only the headline and outcome, never raw agent text.
                        None => short_announcement(&next.event)
                            .unwrap_or_else(|| fallback_summary(&next.event)),
                    };
                    let line = announcement(&next.event, &spoken, next.more_waiting);
                    if v.echo {
                        println!("speaker> {line}");
                    }
                    let delivered = !connected || v.announcer.announce(&line).await;
                    if delivered {
                        v.core.mark_announced(&[next.event.id]);
                    }
                }
                v.polling.store(false, Ordering::Relaxed);
            }
        }
    }
}

/// A spoken summary: from the summary model if possible, else the headline and outcome only.
pub async fn summarise(llm: &Llm, usage: &mut UsageTracker, day: i64, event: &Event) -> String {
    let cfg = SummaryConfig::default();
    if let Some(spoken) = short_announcement(event) {
        return spoken;
    }
    if usage.check(day).is_ok() {
        let (system, user) = summary_prompt(event, &cfg);
        let req = ChatRequest {
            model: llm
                .cfg
                .summary_model
                .clone()
                .unwrap_or_else(|| llm.cfg.model.clone()),
            messages: vec![Message::system(system), Message::user(user)],
            tools: Vec::new(),
            max_tokens: 120,
        };
        let outcome = llm.backend.complete(&req).await;
        match &outcome {
            Ok(c) => usage.record_reply(day, &req, c),
            Err(_) => usage.record_request(day),
        }
        if let Ok(c) = outcome {
            let spoken = clamp_for_speech(&c.text, &cfg);
            if !spoken.is_empty() {
                return spoken;
            }
        }
    }
    fallback_summary(event)
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice")
            .field("echo", &self.echo)
            .field("available", &self.available)
            .field("polling", &self.polling)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Adapters, Limits};
    use crate::sched::{Clock, Kind, Ts};
    use crate::usage::UsageLimits;
    use std::sync::atomic::AtomicI64;
    use std::time::Duration;

    struct Mono(AtomicI64);
    impl Clock for Mono {
        fn now(&self) -> Ts {
            1_800_000_000 + self.0.load(Ordering::Relaxed)
        }
        fn monotonic_secs(&self) -> i64 {
            1 + self.0.load(Ordering::Relaxed)
        }
    }

    struct Rig {
        voice: Arc<Voice>,
        tx: mpsc::UnboundedSender<Say>,
        clock: Arc<Mono>,
    }

    fn rig(waits_for_satellite: bool) -> Rig {
        let clock = Arc::new(Mono(AtomicI64::new(0)));
        let core = Arc::new(Core::in_memory(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
        ));
        let voice = Arc::new(Voice {
            announcer: satellite::Announcer::default(),
            core,
            usage: UsageTracker::new(UsageLimits::default()),
            usage_store: JsonStore::new("unused-usage.json", 1),
            echo: false,
            waits_for_satellite,
            available: AtomicBool::new(false),
            polling: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(speak(Arc::clone(&voice), rx));
        Rig { voice, tx, clock }
    }

    fn rang(kind: Kind, label: &str) -> Fired {
        Fired {
            id: 1,
            kind,
            label: label.to_owned(),
            repeats: false,
            late_secs: 0,
            missed: false,
        }
    }

    fn poll() -> Say {
        Say::Poll {
            minute: 0,
            day: 0,
            llm: None,
        }
    }

    async fn heard(rx: &mut mpsc::UnboundedReceiver<String>) -> Option<String> {
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// Let the speaker task work through what was sent.
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    async fn silence(rx: &mut mpsc::UnboundedReceiver<String>) -> bool {
        tokio::time::timeout(Duration::from_millis(150), rx.recv())
            .await
            .is_err()
    }

    // REQ-TMR-015: a timer that rings with no speaker connected is said when one connects.
    #[tokio::test]
    async fn a_rung_line_waits_for_a_satellite_that_is_meant_to_speak_it() {
        let r = rig(true);
        r.tx.send(Say::Line(rang(Kind::Timer, ""))).unwrap();
        r.tx.send(poll()).unwrap();
        settle().await;
        let free = Arc::new(AtomicBool::new(true));
        let mut ears = r.voice.announcer.connect_fake(free);
        assert!(silence(&mut ears).await, "nothing was connected yet");
        r.tx.send(poll()).unwrap();
        assert_eq!(
            heard(&mut ears).await.as_deref(),
            Some("Your timer is done.")
        );
        r.tx.send(poll()).unwrap();
        assert!(silence(&mut ears).await, "said once, not again");
    }

    // REQ-TMR-015: a satellite that is busy or muted does not lose the line either.
    #[tokio::test]
    async fn a_rung_line_waits_for_a_busy_satellite() {
        let r = rig(true);
        let free = Arc::new(AtomicBool::new(false));
        let mut ears = r.voice.announcer.connect_fake(Arc::clone(&free));
        r.tx.send(Say::Line(rang(Kind::Reminder, "call mum")))
            .unwrap();
        assert!(silence(&mut ears).await);
        free.store(true, Ordering::Relaxed);
        r.tx.send(poll()).unwrap();
        assert_eq!(
            heard(&mut ears).await.as_deref(),
            Some("Reminder: call mum.")
        );
    }

    // REQ-TMR-015: past the grace period the line is worded as missed, by the monotonic clock.
    #[tokio::test]
    async fn a_line_that_waited_too_long_is_said_as_missed() {
        let r = rig(true);
        r.tx.send(Say::Line(rang(Kind::Timer, "tea"))).unwrap();
        r.tx.send(Say::Line(rang(Kind::Reminder, "call mum")))
            .unwrap();
        r.tx.send(poll()).unwrap();
        settle().await;
        r.clock.0.store(MISSED_GRACE_SECS + 1, Ordering::Relaxed);
        settle().await;
        let free = Arc::new(AtomicBool::new(true));
        let mut ears = r.voice.announcer.connect_fake(free);
        assert!(silence(&mut ears).await);
        r.tx.send(poll()).unwrap();
        assert_eq!(
            heard(&mut ears).await.as_deref(),
            Some("A timer for tea finished while I was off.")
        );
        assert_eq!(
            heard(&mut ears).await.as_deref(),
            Some("You missed a reminder: call mum.")
        );
    }

    // REQ-TMR-015: at most 16 lines are kept, the newest, in the order they rang.
    #[tokio::test]
    async fn at_most_sixteen_lines_wait() {
        let r = rig(true);
        for n in 0..20 {
            r.tx.send(Say::Line(rang(Kind::Reminder, &n.to_string())))
                .unwrap();
        }
        r.tx.send(poll()).unwrap();
        settle().await;
        let free = Arc::new(AtomicBool::new(true));
        let mut ears = r.voice.announcer.connect_fake(free);
        assert!(silence(&mut ears).await);
        r.tx.send(poll()).unwrap();
        let mut got = Vec::new();
        while let Some(l) = heard(&mut ears).await {
            got.push(l);
        }
        let want: Vec<String> = (4..20).map(|n| format!("Reminder: {n}.")).collect();
        assert_eq!(got, want);
    }

    // REQ-TMR-015: with no satellite meant to speak (or a terminal echoing speech) nothing is kept.
    #[tokio::test]
    async fn without_a_satellite_to_wait_for_a_line_is_not_kept() {
        let r = rig(false);
        r.tx.send(Say::Line(rang(Kind::Timer, ""))).unwrap();
        r.tx.send(poll()).unwrap();
        settle().await;
        let free = Arc::new(AtomicBool::new(true));
        let mut ears = r.voice.announcer.connect_fake(free);
        assert!(silence(&mut ears).await);
        r.tx.send(poll()).unwrap();
        assert!(silence(&mut ears).await, "best effort: it was dropped");
        // A satellite that is connected and free still hears a line as it rings.
        r.tx.send(Say::Line(rang(Kind::Timer, ""))).unwrap();
        assert_eq!(
            heard(&mut ears).await.as_deref(),
            Some("Your timer is done.")
        );
    }

    // REQ-INB-006: an event is announced when someone can hear it.
    #[test]
    fn an_event_waits_for_a_satellite_that_is_meant_to_speak_it() {
        // A satellite is connected: it must be free.
        assert!(may_announce(true, true, true));
        assert!(!may_announce(true, false, true));
        assert!(!may_announce(true, false, false));
        // None is connected. With one configured, the event waits for it to come back.
        assert!(!may_announce(false, false, true));
        // With none configured (the terminal or the portal is the only place to see it), it goes ahead.
        assert!(may_announce(false, false, false));
    }
}
