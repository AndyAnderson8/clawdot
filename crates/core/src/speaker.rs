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
    /// Some satellite could speak right now, as of the last poll (read by the loop for the portal).
    pub available: AtomicBool,
    /// A poll is queued or running; the loop sends another only when this is false.
    pub polling: AtomicBool,
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
                // A satellite that is connected but busy or muted: keep the event queued and try again.
                if (!connected || free)
                    && let Some(next) = v.core.next_announcement(true, minute)
                {
                    let spoken = match &llm {
                        Some(l) => {
                            let mut usage = v.usage.clone();
                            let s = summarise(l, &mut usage, day, &next.event).await;
                            if let Err(e) = usage.save(&v.usage_store) {
                                crate::diag::error(&format!("could not save usage counters: {e}"));
                            }
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
        usage.record(day, outcome.as_ref().ok().and_then(|c| c.usage.as_ref()));
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
