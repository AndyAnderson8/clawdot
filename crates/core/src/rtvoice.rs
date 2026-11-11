//! The parts of the optional realtime voice (ADR-0020) that the rest of the program touches: its settings, its
//! status, the hand-off between a speaker's connection and the session, and audio conversion.
//!
//! They are always compiled, are small and use no network, so the speaker loop and the settings page do not
//! need the cargo feature `realtime`. The session itself (the WebSocket and the provider's protocol) lives in
//! [`crate::realtime`] and exists only with the feature. Without it, [`RtSettings::configured`] is always
//! false and nothing here ever starts a session (REQ-RTV-001, REQ-RTV-008).

use crate::assistant::Effect;
use serde_json::{Map, Value, json};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{mpsc, oneshot};

/// The only provider there is a dialect for.
pub const PROVIDER: &str = "openai";
/// The environment variable that holds the key unless another is named.
pub const DEFAULT_KEY_ENV: &str = "OPENAI_API_KEY";
/// The default voice.
pub const DEFAULT_VOICE: &str = "alloy";
/// The default speech-recognition model, needed to read a spoken yes or no.
pub const DEFAULT_TRANSCRIPTION: &str = "gpt-4o-mini-transcribe";
/// Default dollars per minute of audio sent. Deliberately above the published rate.
pub const DEFAULT_USD_IN: f64 = 0.03;
/// Default dollars per minute of audio received. Deliberately above the published rate.
pub const DEFAULT_USD_OUT: f64 = 0.10;
/// Default longest session, in seconds.
pub const DEFAULT_MAX_SESSION_SECS: u64 = 300;

/// What the engine sends to the speaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Speaker {
    /// Audio to play (PCM16, 24 kHz).
    Audio(Vec<u8>),
    /// Drop what is queued: the user began speaking over the model.
    Flush,
}

/// Linear resampling of 16-bit mono PCM, for example a 16 kHz microphone to 24 kHz.
#[must_use]
pub fn resample(pcm: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || pcm.is_empty() || from == 0 || to == 0 {
        return pcm.to_vec();
    }
    let n = (pcm.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * f64::from(from) / f64::from(to);
            let i0 = (pos as usize).min(pcm.len() - 1);
            let i1 = (i0 + 1).min(pcm.len() - 1);
            let frac = pos - i0 as f64;
            (f64::from(pcm[i0]) * (1.0 - frac) + f64::from(pcm[i1]) * frac).round() as i16
        })
        .collect()
}

/// The realtime settings in effect.
#[derive(Clone, PartialEq)]
pub struct RtSettings {
    /// The administrator turned it on.
    pub enabled: bool,
    /// `wss://` address of the service.
    pub url: Option<String>,
    /// Model name.
    pub model: Option<String>,
    /// Voice name.
    pub voice: String,
    /// Name of the environment variable that holds the key.
    pub key_env: String,
    /// Model that transcribes what the user says, or empty for none.
    pub transcription_model: String,
    /// Dollars per minute of audio sent.
    pub usd_per_min_in: f64,
    /// Dollars per minute of audio received.
    pub usd_per_min_out: f64,
    /// Longest a session may last.
    pub max_session_secs: u64,
}

impl Default for RtSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            url: None,
            model: None,
            voice: DEFAULT_VOICE.to_owned(),
            key_env: DEFAULT_KEY_ENV.to_owned(),
            transcription_model: DEFAULT_TRANSCRIPTION.to_owned(),
            usd_per_min_in: DEFAULT_USD_IN,
            usd_per_min_out: DEFAULT_USD_OUT,
            max_session_secs: DEFAULT_MAX_SESSION_SECS,
        }
    }
}

impl fmt::Debug for RtSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtSettings")
            .field("enabled", &self.enabled)
            .field("url", &self.url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl RtSettings {
    /// Whether this build can run a realtime session at all (the cargo feature `realtime`).
    #[must_use]
    pub const fn built() -> bool {
        cfg!(feature = "realtime")
    }

    /// Read the `realtime_*` keys among the live settings. A missing key keeps the default.
    #[must_use]
    pub fn from_live(live: &Map<String, Value>) -> Self {
        let d = Self::default();
        let text = |k: &str| {
            live.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let num = |k: &str, or: f64| live.get(k).and_then(Value::as_f64).unwrap_or(or);
        Self {
            enabled: live
                .get("realtime_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            url: text("realtime_url"),
            model: text("realtime_model"),
            voice: text("realtime_voice").unwrap_or(d.voice),
            key_env: text("realtime_key_env").unwrap_or(d.key_env),
            transcription_model: live
                .get("realtime_transcription_model")
                .and_then(Value::as_str)
                .map_or(d.transcription_model, str::to_owned),
            usd_per_min_in: num("realtime_usd_per_min_in", d.usd_per_min_in),
            usd_per_min_out: num("realtime_usd_per_min_out", d.usd_per_min_out),
            max_session_secs: live
                .get("realtime_max_session_secs")
                .and_then(Value::as_u64)
                .unwrap_or(d.max_session_secs),
        }
    }

    /// The key: one pasted into the portal wins, otherwise the environment variable that was named. `env` looks
    /// a variable up.
    #[must_use]
    pub fn key_with(&self, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
        crate::secrets::get(crate::secrets::REALTIME_KEY)
            .or_else(|| env(&self.key_env))
            .filter(|k| !k.is_empty())
    }

    /// [`Self::key_with`] using the process environment.
    #[must_use]
    pub fn key(&self) -> Option<String> {
        self.key_with(&|n| std::env::var(n).ok())
    }

    /// Ready to start a session: built in, chosen, addressed, and with a key.
    #[must_use]
    pub fn configured_with(&self, env: &dyn Fn(&str) -> Option<String>) -> bool {
        Self::built()
            && self.enabled
            && self.url.is_some()
            && self.model.is_some()
            && self.key_with(env).is_some()
    }

    /// [`Self::configured_with`] using the process environment.
    #[must_use]
    pub fn configured(&self) -> bool {
        self.configured_with(&|n| std::env::var(n).ok())
    }
}

/// How a session ended, as the speaker needs to know.
#[derive(Debug)]
pub struct Done {
    /// Something to say to the user (an explanation, or why the normal voice is used instead).
    pub speak: Option<String>,
    /// The session could not run or broke; the speaker uses the normal voice for a while (REQ-RTV-008).
    pub failed: bool,
    /// What the tools asked the speaker to do during the session (volume, music).
    pub effects: Vec<Effect>,
}

/// One realtime session for the core to run on a speaker's behalf. Closing `mic` ends it.
#[derive(Debug)]
pub struct Job {
    /// Microphone audio from the speaker, PCM16 at 24 kHz.
    pub mic: mpsc::Receiver<Vec<u8>>,
    /// Where the model's audio goes.
    pub speaker: mpsc::Sender<Speaker>,
    /// How it ended.
    pub done: oneshot::Sender<Done>,
}

struct Inner {
    settings: Mutex<RtSettings>,
    active: AtomicBool,
    last_error: Mutex<Option<String>>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
}

/// The realtime voice as the rest of the program sees it: the current settings, whether a session is open, the
/// last error, and the way to hand a session to the core. Cheap to clone.
#[derive(Clone)]
pub struct Realtime(Arc<Inner>);

impl fmt::Debug for Realtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Realtime")
    }
}

impl Default for Realtime {
    fn default() -> Self {
        Self::new(RtSettings::default())
    }
}

impl Realtime {
    /// A handle with these settings and nowhere to send sessions yet.
    #[must_use]
    pub fn new(settings: RtSettings) -> Self {
        Self(Arc::new(Inner {
            settings: Mutex::new(settings),
            active: AtomicBool::new(false),
            last_error: Mutex::new(None),
            jobs: Mutex::new(None),
        }))
    }

    /// Switch to new settings (a live change from the portal).
    pub fn set_settings(&self, s: RtSettings) {
        *self
            .0
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = s;
    }

    /// The settings in effect.
    #[must_use]
    pub fn settings(&self) -> RtSettings {
        self.0
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The core takes sessions from the returned receiver. Call once.
    #[must_use]
    pub fn attach(&self) -> mpsc::Receiver<Job> {
        let (tx, rx) = mpsc::channel(1);
        *self.0.jobs.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        rx
    }

    /// Record (or clear) the last error. It is one plain line and holds no key and no audio.
    pub fn set_error(&self, e: Option<String>) {
        *self
            .0
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = e;
    }

    /// Claim the right to run a session, if realtime is ready and none is open. Hold the claim as long as the
    /// session lasts.
    #[must_use]
    pub fn claim(&self) -> Option<Claim> {
        if !self.settings().configured() {
            return None;
        }
        let tx = self
            .0
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        self.0
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(Claim {
            rt: self.clone(),
            tx,
        })
    }

    /// What the status snapshot carries: `{configured, active, last_error}`. No key, no audio, no text said.
    #[must_use]
    pub fn status(&self) -> Value {
        json!({
            "configured": self.settings().configured(),
            "active": self.0.active.load(Ordering::Acquire),
            "last_error": self.0.last_error.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        })
    }
}

/// The right to run one session. Dropping it frees the slot.
pub struct Claim {
    rt: Realtime,
    tx: mpsc::Sender<Job>,
}

impl fmt::Debug for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Claim")
    }
}

impl Claim {
    /// Hand the session to the core without waiting. False when the core is not taking one.
    #[must_use]
    pub fn submit(&self, job: Job) -> bool {
        self.tx.try_send(job).is_ok()
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.rt.0.active.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn the_settings_are_read_from_the_live_values_with_defaults() {
        let s = RtSettings::from_live(&live(&[
            ("realtime_enabled", json!(true)),
            ("realtime_url", json!("wss://x.example/v1/realtime")),
            ("realtime_model", json!("m")),
            ("realtime_voice", json!("verse")),
            ("realtime_usd_per_min_in", json!(0.5)),
            ("realtime_max_session_secs", json!(60)),
            ("realtime_transcription_model", json!("")),
        ]));
        assert!(s.enabled && s.url.is_some() && s.model.as_deref() == Some("m"));
        assert_eq!(
            (s.voice.as_str(), s.usd_per_min_in, s.max_session_secs),
            ("verse", 0.5, 60)
        );
        assert_eq!(s.usd_per_min_out, DEFAULT_USD_OUT);
        assert_eq!(s.key_env, DEFAULT_KEY_ENV);
        assert_eq!(s.transcription_model, "", "an empty model means none");
        assert!(!RtSettings::from_live(&Map::new()).enabled);
    }

    // REQ-RTV-001, REQ-RTV-008
    #[test]
    fn it_is_configured_only_when_built_chosen_addressed_and_keyed() {
        let mut s = RtSettings {
            enabled: true,
            url: Some("wss://x/v1".into()),
            model: Some("m".into()),
            ..RtSettings::default()
        };
        let with_key = |_: &str| Some("sk-test".to_owned());
        let no_key = |_: &str| None;
        assert_eq!(s.configured_with(&with_key), RtSettings::built());
        assert!(!s.configured_with(&no_key));
        s.enabled = false;
        assert!(!s.configured_with(&with_key));
        let shown = format!("{s:?}");
        assert!(!shown.contains("sk-"), "{shown}");
    }

    #[test]
    fn status_starts_unconfigured_and_keeps_the_last_error() {
        let rt = Realtime::default();
        assert_eq!(
            rt.status(),
            json!({"configured": false, "active": false, "last_error": null})
        );
        assert!(rt.claim().is_none(), "not configured, so nothing to claim");
        rt.set_error(Some(
            "The realtime voice service did not accept the API key.".into(),
        ));
        assert_eq!(
            rt.status()["last_error"],
            "The realtime voice service did not accept the API key."
        );
    }

    #[test]
    fn resampling_changes_the_length_by_the_rate_ratio() {
        let pcm: Vec<i16> = (0..160).collect();
        assert_eq!(resample(&pcm, 16_000, 24_000).len(), 240);
        assert_eq!(
            resample(&resample(&pcm, 16_000, 24_000), 24_000, 16_000).len(),
            160
        );
        assert_eq!(resample(&pcm, 16_000, 16_000), pcm);
        assert!(resample(&[], 16_000, 24_000).is_empty());
    }
}
