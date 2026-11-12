//! The speaker task: everything that talks (portal phrases, timer lines, agent-event announcements) is handed
//! to it and said in order, so waiting for a voice or a summary never holds the main loop (ADR-0021).

use crate::app::Core;
use crate::inbox::event::Event;
use crate::inbox::summary::{
    SummaryConfig, announcement, clamp_for_speech, fallback_summary, short_announcement,
    summary_prompt,
};
use crate::llm::{ChatRequest, Message};
use crate::runtime::Llm;
use crate::satellite;
use crate::session::ChatBackend;
use crate::store::JsonStore;
use crate::usage::UsageTracker;
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
    /// A timer, alarm or reminder that rang.
    Line(String),
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

/// Say what the loop hands over, one thing at a time. A satellite that is busy or muted simply does not
/// speak a timer's line (best effort); an agent event stays queued until one can.
pub async fn speak(voice: Arc<Voice>, mut rx: mpsc::UnboundedReceiver<Say>) {
    let v = &*voice;
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
            Say::Line(line) => {
                if v.echo {
                    println!("speaker> {line}");
                }
                if v.announcer.connected() > 0 {
                    v.announcer.announce(&line).await;
                }
            }
            Say::Poll { minute, day, llm } => {
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
    use super::may_announce;

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
