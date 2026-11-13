//! The single human-editable configuration file (REQ-CFG-007).
//!
//! JSON, so the core needs no extra parser. Every key is optional; command-line flags override the file.
//! Unknown keys are errors, so a typo is reported at startup instead of silently ignored. Secrets are never
//! stored here: the provider key comes from an environment variable named by `llm.key_env`.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Language-model settings.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    /// OpenAI-compatible base URL, for example `https://openrouter.ai/api/v1`.
    pub url: Option<String>,
    /// Model used for conversation and tool calls.
    pub model: Option<String>,
    /// Retired: nothing uses a second model any more. Still accepted so an older file loads, and ignored.
    pub fallback_model: Option<String>,
    /// Retired: summaries use `model`. Still accepted so an older file loads, and ignored.
    pub summary_model: Option<String>,
    /// Name of the environment variable that holds the API key.
    pub key_env: Option<String>,
}

/// Usage caps.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimitsConfig {
    /// Retired: there is no cap on the number of requests any more. Still accepted so an older file loads, and ignored.
    pub max_requests_per_day: Option<u32>,
    /// Spend per local day in US dollars; must not be negative.
    pub daily_spend_usd: Option<f64>,
    /// Actions (calls that are not read-only) per local day; 0 stops all actions (default 100, REQ-SEC-015).
    pub max_actions_per_day: Option<u32>,
    /// Costly and destructive calls per local day (default 20, REQ-SEC-015).
    pub max_costly_per_day: Option<u32>,
    /// Describe actions instead of running them (default off, REQ-SEC-016).
    pub dry_run: Option<bool>,
}

/// How a turn behaves (REQ-AST-019 to REQ-AST-025). Every value can be lowered; a value above the ceiling is
/// clamped to it.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Most model rounds in one turn (default 8, ceiling 20).
    pub max_rounds: Option<usize>,
    /// Most tool calls in one turn (default 12, ceiling 20).
    pub max_tool_calls: Option<usize>,
    /// Seconds one turn may take (default 60, ceiling 300).
    pub deadline_secs: Option<u64>,
    /// Seconds one tool call may take before it counts as failed (default 45, at most 120).
    pub tool_timeout_secs: Option<u64>,
    /// Say the plan before multi-step work (default true).
    pub plan_speech: Option<bool>,
    /// Say short progress phrases while tools run (default true).
    pub progress_speech: Option<bool>,
    /// Summarise tool results longer than this many characters; 0 turns it off (default 400).
    pub summarize_over_chars: Option<usize>,
    /// Keep notes the user asks for (default false, REQ-AST-023).
    pub notes: Option<bool>,
}

/// Update checks (REQ-UPD-001 to REQ-UPD-010).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UpdatesConfig {
    /// Look for a newer release about once a day (default true; the person can change it in the portal).
    pub check: Option<bool>,
    /// Where releases are listed (default: the project's releases on GitHub). `https://` only.
    pub url: Option<String>,
    /// The maintainer's Ed25519 public key as 64 hexadecimal digits. Without one a newer version is announced but
    /// never installed by ClawDot itself.
    pub public_key: Option<String>,
}

/// Speech-to-text and text-to-speech provider (OpenAI-compatible).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpeechConfig {
    /// Whose wire format to speak: `openai` (the default, and every OpenAI-compatible server), `elevenlabs` or
    /// `deepgram`. The last two have a default `url`, models and voice (REQ-VOI-021).
    pub provider: Option<String>,
    /// Base URL, for example `https://api.openai.com/v1`. Plain `http://` only for local hosts.
    pub url: Option<String>,
    /// Name of the environment variable that holds the API key.
    pub key_env: Option<String>,
    /// Transcription model, for example `whisper-1`.
    pub stt_model: Option<String>,
    /// Speech model, for example `tts-1`.
    pub tts_model: Option<String>,
    /// Voice name.
    pub voice: Option<String>,
    /// Spoken language hint for transcription, for example `en`.
    pub language: Option<String>,
    /// Sample rate of the provider's raw PCM output (default 24000).
    pub tts_sample_rate: Option<u32>,
}

/// Optional realtime (speech-to-speech) voice (ADR-0020, REQ-RTV-008). Needs a build with the `realtime`
/// feature. The key is never here: it is read from the environment variable `key_env` names, or the key pasted
/// into the portal.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RealtimeSection {
    /// Use realtime voice for a turn after the wake word (default false).
    pub enabled: Option<bool>,
    /// The provider; only `openai` (the OpenAI Realtime protocol) exists.
    pub provider: Option<String>,
    /// `wss://` address, for example `wss://api.openai.com/v1/realtime`. Plain `ws://` only for this machine or
    /// the home network.
    pub url: Option<String>,
    /// Model name.
    pub model: Option<String>,
    /// Voice name (default `alloy`).
    pub voice: Option<String>,
    /// Name of the environment variable that holds the API key (default `OPENAI_API_KEY`).
    pub key_env: Option<String>,
    /// Model that transcribes what you say so a spoken yes or no can be read; empty for none.
    pub transcription_model: Option<String>,
    /// Dollars per minute of audio sent (default 0.03).
    pub usd_per_min_in: Option<f64>,
    /// Dollars per minute of audio received (default 0.10).
    pub usd_per_min_out: Option<f64>,
    /// Longest a session may last, in seconds (default 300).
    pub max_session_secs: Option<u64>,
}

/// Voice settings beyond the speech provider.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VoiceConfig {
    /// Realtime voice (optional feature `realtime`).
    #[serde(default)]
    pub realtime: RealtimeSection,
}

impl RealtimeSection {
    fn validate(&self) -> Result<(), String> {
        let bad = |m: &str| Err(format!("config: {m}"));
        if let Some(p) = &self.provider
            && p != crate::rtvoice::PROVIDER
        {
            return bad("`voice.realtime.provider` must be `openai`");
        }
        if self.url.is_some() != self.model.is_some() {
            return bad("`voice.realtime.url` and `voice.realtime.model` must be set together");
        }
        if let Some(u) = &self.url
            && (u.contains(char::is_whitespace)
                || !(u.starts_with("wss://") || u.starts_with("ws://")))
        {
            return bad(
                "`voice.realtime.url` must start with wss:// (or ws:// on this machine or your own network)",
            );
        }
        if let Some(u) = &self.url {
            crate::netpolicy::check_realtime_url(u)
                .map_err(|e| format!("config: `voice.realtime.url`: {e}"))?;
        }
        if let Some(n) = &self.key_env
            && !crate::settings::is_env_name(n)
        {
            return bad(
                "`voice.realtime.key_env` is the NAME of an environment variable, not the key",
            );
        }
        for (name, v) in [
            ("usd_per_min_in", self.usd_per_min_in),
            ("usd_per_min_out", self.usd_per_min_out),
        ] {
            if v.is_some_and(|x| !x.is_finite() || !(0.0..=100.0).contains(&x)) {
                return Err(format!(
                    "config: `voice.realtime.{name}` must be a number of dollars from 0 to 100"
                ));
            }
        }
        if self
            .max_session_secs
            .is_some_and(|s| !(10..=3600).contains(&s))
        {
            return bad("`voice.realtime.max_session_secs` must be from 10 to 3600");
        }
        if self.enabled == Some(true) {
            if self.url.is_none() {
                return bad(
                    "`voice.realtime.enabled` needs `voice.realtime.url` and `voice.realtime.model`",
                );
            }
            if !crate::rtvoice::RtSettings::built() {
                return bad("`voice.realtime.enabled` needs a build with the `realtime` feature");
            }
        }
        Ok(())
    }
}

impl SpeechConfig {
    /// The dialect named by `provider` (the OpenAI style when unset or unknown; `validate` rejects unknown names).
    #[must_use]
    pub fn dialect(&self) -> crate::speech::Dialect {
        self.provider
            .as_deref()
            .and_then(|p| crate::speech::Dialect::parse(p).ok())
            .unwrap_or_default()
    }

    /// Fill what the chosen service already knows: its address, models and voice. Settings the person gave win.
    pub fn apply_provider_defaults(&mut self) {
        let d = self.dialect();
        let fill = |slot: &mut Option<String>, v: Option<&str>| {
            if slot.is_none() {
                *slot = v.map(str::to_owned);
            }
        };
        fill(&mut self.url, d.default_url());
        fill(&mut self.stt_model, d.default_stt_model());
        fill(&mut self.tts_model, d.default_tts_model());
        fill(&mut self.voice, d.default_voice());
        fill(&mut self.key_env, d.default_key_env());
    }
}

/// The satellite server: where speakers and other audio devices connect.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SatelliteConfig {
    /// Address to accept satellites on, for example `0.0.0.0:10400`. Off when unset.
    pub listen: Option<String>,
    /// Name of the environment variable holding the shared satellite token. Required unless `listen` is
    /// a loopback address.
    pub token_env: Option<String>,
    /// Seconds after a reply during which listening stays open without the wake word (default 8; 0 = off).
    pub follow_up_secs: Option<u64>,
    /// How the microphone endpointer is tuned for these satellites (REQ-VOI-025).
    #[serde(default)]
    pub listening: ListeningConfig,
}

/// Tuning for the endpointer that finds where an utterance starts and ends (REQ-VOI-025). Every key is optional
/// and falls back to the default in [`crate::vad::Config`], which suits a quiet room and a close microphone. A
/// device with a different microphone gain, or a far-field array, sets what it needs. The same keys are read
/// in `satellite` and in each ESPHome device entry; the device's own values win.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ListeningConfig {
    /// Quietest level (RMS of 16-bit samples) that can count as speech: 1 to 10000 (default 400).
    pub min_level: Option<f32>,
    /// Speech must be this many times louder than the background: 1 to 20 (default 3).
    pub noise_ratio: Option<f32>,
    /// Consecutive loud 20 ms frames that start an utterance: 1 to 50 (default 3).
    pub start_frames: Option<usize>,
    /// Silence that ends an utterance, in milliseconds: 100 to 5000 (default 700).
    pub silence_ms: Option<usize>,
    /// Shorter utterances are treated as noise, in milliseconds: 20 to 2000 (default 250).
    pub min_speech_ms: Option<usize>,
    /// Utterances are cut at this length, in milliseconds: 1000 to 60000 (default 15000).
    pub max_utterance_ms: Option<usize>,
    /// Give up when nobody speaks within this time after a wake, in milliseconds: 1000 to 60000 (default 6000).
    pub no_speech_ms: Option<usize>,
    /// Audio kept from before speech was detected, in milliseconds: 0 to 2000 (default 300).
    pub preroll_ms: Option<usize>,
}

impl ListeningConfig {
    /// `base` with the keys set here replacing its values.
    #[must_use]
    pub fn apply(&self, base: crate::vad::Config) -> crate::vad::Config {
        crate::vad::Config {
            min_level: self.min_level.unwrap_or(base.min_level),
            noise_ratio: self.noise_ratio.unwrap_or(base.noise_ratio),
            start_frames: self.start_frames.unwrap_or(base.start_frames),
            silence_ms: self.silence_ms.unwrap_or(base.silence_ms),
            min_speech_ms: self.min_speech_ms.unwrap_or(base.min_speech_ms),
            max_utterance_ms: self.max_utterance_ms.unwrap_or(base.max_utterance_ms),
            no_speech_ms: self.no_speech_ms.unwrap_or(base.no_speech_ms),
            preroll_ms: self.preroll_ms.unwrap_or(base.preroll_ms),
        }
    }

    /// Check each value against its range. `place` names the object for the message, for example
    /// `satellite.listening`.
    ///
    /// # Errors
    /// A message naming the key and the range.
    pub fn validate(&self, place: &str) -> Result<(), String> {
        let float = |key: &str, v: Option<f32>, lo: f32, hi: f32| match v {
            Some(v) if !(v.is_finite() && (lo..=hi).contains(&v)) => Err(format!(
                "config: `{place}.{key}` must be a number from {lo} to {hi}"
            )),
            _ => Ok(()),
        };
        let whole = |key: &str, v: Option<usize>, lo: usize, hi: usize| match v {
            Some(v) if !(lo..=hi).contains(&v) => Err(format!(
                "config: `{place}.{key}` must be a whole number from {lo} to {hi}"
            )),
            _ => Ok(()),
        };
        float("min_level", self.min_level, 1.0, 10_000.0)?;
        float("noise_ratio", self.noise_ratio, 1.0, 20.0)?;
        whole("start_frames", self.start_frames, 1, 50)?;
        whole("silence_ms", self.silence_ms, 100, 5_000)?;
        whole("min_speech_ms", self.min_speech_ms, 20, 2_000)?;
        whole("max_utterance_ms", self.max_utterance_ms, 1_000, 60_000)?;
        whole("no_speech_ms", self.no_speech_ms, 1_000, 60_000)?;
        whole("preroll_ms", self.preroll_ms, 0, 2_000)
    }

    /// Check the values that only make sense together, after these keys replace `base`: an utterance must be able
    /// to be longer than the shortest one that counts.
    ///
    /// # Errors
    /// A message naming the two keys.
    pub fn validate_combined(&self, base: crate::vad::Config, place: &str) -> Result<(), String> {
        let v = self.apply(base);
        if v.min_speech_ms >= v.max_utterance_ms {
            return Err(format!(
                "config: `{place}.min_speech_ms` must be less than `max_utterance_ms`"
            ));
        }
        Ok(())
    }
}

/// Parts the operating system cannot show, or shows wrongly, declared by the person who set the device up. Each is
/// `true` (it is there), `false` (ignore what was detected) or absent (as detected).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PartsConfig {
    /// A ring or row of lights, for example one that another service drives so the kernel lists no LED.
    pub leds: Option<bool>,
    /// A Bluetooth adapter.
    pub bluetooth: Option<bool>,
    /// A wireless network adapter.
    pub wifi: Option<bool>,
    /// A screen.
    pub screen: Option<bool>,
    /// A USB serial adapter.
    pub usb_serial: Option<bool>,
}

/// What this device is called and what its operating system cannot tell the core about it (REQ-CFG-083). The core
/// knows no device by name: a preset for a particular device is data in this section, not code.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    /// A short identifier for the kind of device, for example the `board` value of a file in `boards/`. Lower-case
    /// letters, digits, `-` and `_`, at most 40. Reported as `hardware.board`; `unknown` when not set.
    pub board: Option<String>,
    /// What to call the device when showing it to a person, at most 60 characters on one line.
    pub label: Option<String>,
    /// Parts to declare or rule out.
    #[serde(default)]
    pub parts: PartsConfig,
}

impl DeviceConfig {
    fn validate(&self) -> Result<(), String> {
        if let Some(b) = &self.board
            && (b.is_empty()
                || b.len() > 40
                || !b.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'
                }))
        {
            return Err("config: `device.board` must be 1 to 40 characters: lower-case letters, digits, - and _".into());
        }
        if let Some(l) = &self.label
            && (l.trim().is_empty() || l.chars().count() > 60 || l.chars().any(char::is_control))
        {
            return Err(
                "config: `device.label` must be 1 to 60 characters with no line breaks".into(),
            );
        }
        Ok(())
    }
}

/// The whole file.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where state is kept.
    pub data_dir: Option<PathBuf>,
    /// Address to serve on, for example `0.0.0.0:8080`.
    pub listen: Option<String>,
    /// URL used in the integration snippets.
    pub public_url: Option<String>,
    /// Device name shown on Home (default: ClawDot, never the host name).
    pub name: Option<String>,
    /// IANA time zone, for example `America/Chicago`.
    pub zone: Option<String>,
    /// File holding the assistant's persona text.
    pub persona_file: Option<PathBuf>,
    /// Inbox endpoints to create at startup, as `NAME` or `NAME:ADAPTER`.
    #[serde(default)]
    pub endpoints: Vec<String>,
    /// Language model.
    #[serde(default)]
    pub llm: LlmConfig,
    /// Usage caps.
    #[serde(default)]
    pub limits: LimitsConfig,
    /// How a turn behaves: limits, speech while working, notes.
    #[serde(default)]
    pub agent: AgentConfig,
    /// Speech provider.
    #[serde(default)]
    pub speech: SpeechConfig,
    /// Voice settings beyond the speech provider (realtime voice).
    #[serde(default)]
    pub voice: VoiceConfig,
    /// Satellite server.
    #[serde(default)]
    pub satellite: SatelliteConfig,
    /// What the device is called and which parts it has that cannot be seen.
    #[serde(default)]
    pub device: DeviceConfig,
    /// Update checks.
    #[serde(default)]
    pub updates: UpdatesConfig,
    /// ESPHome voice satellites to dial (optional feature `esphome-satellite`; see spec 0004).
    #[cfg(feature = "esphome-satellite")]
    #[serde(default)]
    pub esphome: crate::esphome_cfg::EsphomeConfig,
    /// Without the feature the section is kept unparsed (so the default build carries no code for it) and
    /// refused by `validate` unless it is empty.
    #[cfg(not(feature = "esphome-satellite"))]
    #[serde(default)]
    pub esphome: Option<serde_json::Value>,
    /// Radio stations for the music commands.
    #[serde(default)]
    pub stations: Vec<crate::media::Station>,
    /// Declarative HTTP tools the model may call (see `docs/architecture/http-tools.md`).
    #[serde(default)]
    pub tools: Vec<crate::httptools::ToolDef>,
    /// MCP servers whose tools the model may call (Streamable HTTP; see `docs/architecture/mcp.md`).
    #[serde(default)]
    pub mcp: Vec<crate::mcp::ServerDef>,
}

impl Config {
    /// Parse and validate. Errors name the problem in plain words.
    ///
    /// # Errors
    /// Returns a message for malformed JSON, unknown keys, and values that cannot work.
    pub fn parse(text: &str) -> Result<Self, String> {
        let c: Self = serde_json::from_str(text).map_err(|e| format!("config: {e}"))?;
        c.validate()?;
        Ok(c)
    }

    /// The listen address, if one is configured.
    ///
    /// # Errors
    /// Returns a message when it is not a socket address.
    pub fn listen_addr(&self) -> Result<Option<SocketAddr>, String> {
        self.listen
            .as_deref()
            .map(|s| {
                s.parse().map_err(|e| {
                    format!("config: `listen` is not an address like 127.0.0.1:8080: {e}")
                })
            })
            .transpose()
    }

    fn validate(&self) -> Result<(), String> {
        self.validate_identity()?;
        self.validate_llm()?;
        self.validate_agent_and_limits()?;
        self.validate_satellite()?;
        self.validate_speech()?;
        self.validate_lists()
    }

    fn validate_identity(&self) -> Result<(), String> {
        self.listen_addr()?;
        self.device.validate()?;
        if let Some(n) = &self.name
            && (n.trim().is_empty() || n.chars().count() > 40 || n.chars().any(char::is_control))
        {
            return Err("config: `name` must be 1 to 40 characters with no line breaks".into());
        }
        Ok(())
    }

    fn validate_llm(&self) -> Result<(), String> {
        if self.llm.url.is_some() != self.llm.model.is_some() {
            return Err("config: `llm.url` and `llm.model` must be set together".into());
        }
        if let Some(u) = &self.llm.url
            && !(u.starts_with("http://") || u.starts_with("https://"))
        {
            return Err("config: `llm.url` must start with http:// or https://".into());
        }
        if self.llm.url.is_none()
            && (self.llm.fallback_model.is_some() || self.llm.summary_model.is_some())
        {
            return Err("config: `llm.fallback_model` and `llm.summary_model` need `llm.url` and `llm.model`".into());
        }
        Ok(())
    }

    fn validate_agent_and_limits(&self) -> Result<(), String> {
        for (name, v) in [
            ("max_rounds", self.agent.max_rounds.map(|n| n as u64)),
            (
                "max_tool_calls",
                self.agent.max_tool_calls.map(|n| n as u64),
            ),
            ("deadline_secs", self.agent.deadline_secs),
            ("tool_timeout_secs", self.agent.tool_timeout_secs),
        ] {
            if v == Some(0) {
                return Err(format!("config: `agent.{name}` must be at least 1"));
            }
        }
        if let Some(s) = self.limits.daily_spend_usd
            && (!s.is_finite() || s < 0.0)
        {
            return Err("config: `limits.daily_spend_usd` must be zero or more".into());
        }
        Ok(())
    }

    /// The network satellite and the ESPHome satellites, both of which need the `speech` section.
    fn validate_satellite(&self) -> Result<(), String> {
        if let Some(l) = &self.satellite.listen {
            let addr: SocketAddr = l.parse().map_err(|e| {
                format!("config: `satellite.listen` is not an address like 0.0.0.0:10400: {e}")
            })?;
            if self.speech.url.is_none() && self.speech.dialect().default_url().is_none() {
                return Err("config: `satellite.listen` needs the `speech` section".into());
            }
            if !addr.ip().is_loopback() && self.satellite.token_env.is_none() {
                return Err("config: `satellite.token_env` is required unless `satellite.listen` is a loopback address".into());
            }
        } else if self.satellite.token_env.is_some() || self.satellite.follow_up_secs.is_some() {
            return Err(
                "config: `satellite.listen` is needed for the other `satellite` settings".into(),
            );
        }
        self.satellite.listening.validate("satellite.listening")?;
        self.satellite
            .listening
            .validate_combined(crate::vad::Config::default(), "satellite.listening")?;
        #[cfg(feature = "esphome-satellite")]
        {
            self.esphome.validate()?;
            let base = self
                .satellite
                .listening
                .apply(crate::vad::Config::default());
            for d in &self.esphome.devices {
                let place = format!("esphome.devices[{}].listening", d.name);
                d.listening.validate_combined(base, &place)?;
            }
            if !self.esphome.is_empty()
                && self.speech.url.is_none()
                && self.speech.dialect().default_url().is_none()
            {
                return Err("config: `esphome` needs the `speech` section".into());
            }
        }
        #[cfg(not(feature = "esphome-satellite"))]
        if self
            .esphome
            .as_ref()
            .is_some_and(|v| !v.as_object().is_some_and(serde_json::Map::is_empty))
        {
            return Err(
                "config: the `esphome` section needs a build with the `esphome-satellite` feature"
                    .into(),
            );
        }
        Ok(())
    }

    fn validate_speech(&self) -> Result<(), String> {
        let sp = &self.speech;
        let dialect = sp
            .provider
            .as_deref()
            .map_or(
                Ok(crate::speech::Dialect::OpenAi),
                crate::speech::Dialect::parse,
            )
            .map_err(|e| format!("config: `speech.provider`: {e}"))?;
        if dialect.default_url().is_some() {
            // A service with its own address has its own models and voice too; none of the rest is required.
        } else if sp.url.is_some() {
            for (key, v) in [
                ("stt_model", &sp.stt_model),
                ("tts_model", &sp.tts_model),
                ("voice", &sp.voice),
            ] {
                if v.is_none() {
                    return Err(format!(
                        "config: `speech.{key}` is needed when `speech.url` is set"
                    ));
                }
            }
        } else if sp.stt_model.is_some()
            || sp.tts_model.is_some()
            || sp.voice.is_some()
            || sp.key_env.is_some()
        {
            return Err("config: `speech.url` is needed for the other `speech` settings".into());
        }
        if sp.tts_sample_rate.is_some_and(|r| r < 8_000) {
            return Err("config: `speech.tts_sample_rate` must be at least 8000".into());
        }
        Ok(())
    }

    /// Realtime voice, stations, MCP servers, HTTP tools and endpoint names.
    fn validate_lists(&self) -> Result<(), String> {
        self.voice.realtime.validate()?;
        for s in &self.stations {
            s.validate().map_err(|e| format!("config: {e}"))?;
        }
        for m in &self.mcp {
            m.validate().map_err(|e| format!("config: {e}"))?;
        }
        for t in &self.tools {
            t.validate().map_err(|e| format!("config: {e}"))?;
        }
        if let Some(e) = self.endpoints.iter().find(|e| e.trim().is_empty()) {
            return Err(format!("config: an `endpoints` entry is empty ({e:?})"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_valid() {
        assert_eq!(Config::parse("{}").unwrap(), Config::default());
    }

    #[test]
    fn a_full_file_parses() {
        let c = Config::parse(
            r#"{
              "data_dir": "/var/lib/clawdot", "listen": "0.0.0.0:8080", "zone": "America/Chicago",
              "public_url": "http://dot.local:8080", "persona_file": "persona.txt", "name": "Kitchen",
              "endpoints": ["claude-code", "ci:generic"],
              "llm": {"url": "https://openrouter.ai/api/v1", "model": "a/b", "fallback_model": "c/d",
                      "summary_model": "e/f", "key_env": "MY_KEY"},
              "limits": {"daily_spend_usd": 1.5}
            }"#,
        )
        .unwrap();
        assert_eq!(c.listen_addr().unwrap().unwrap().port(), 8080);
        assert_eq!(c.endpoints.len(), 2);
    }

    // REQ-SEC-015, REQ-SEC-016
    #[test]
    fn action_caps_and_dry_run_are_configurable() {
        let c = Config::parse(
            r#"{"limits": {"max_actions_per_day": 30, "max_costly_per_day": 5, "dry_run": true}}"#,
        )
        .unwrap();
        assert_eq!(
            (
                c.limits.max_actions_per_day,
                c.limits.max_costly_per_day,
                c.limits.dry_run
            ),
            (Some(30), Some(5), Some(true))
        );
        assert!(Config::parse(r#"{"limits": {"dry_run": "yes"}}"#).is_err());
        assert!(Config::parse(r#"{"limits": {"max_actions_per_day": -1}}"#).is_err());
    }

    // REQ-ESP-002
    #[cfg(not(feature = "esphome-satellite"))]
    #[test]
    fn req_esp_002_a_default_build_refuses_a_real_esphome_section() {
        assert!(Config::parse(r#"{"esphome": {}}"#).is_ok());
        let e = Config::parse(r#"{"esphome": {"devices": [{"name": "a", "address": "h"}]}}"#)
            .unwrap_err();
        assert!(e.contains("esphome-satellite"), "{e}");
    }

    // REQ-ESP-002
    #[cfg(feature = "esphome-satellite")]
    #[test]
    fn req_esp_002_the_esphome_section_needs_speech() {
        let dev = r#""esphome": {"devices": [{"name": "a", "address": "h"}]}"#;
        let e = Config::parse(&format!("{{{dev}}}")).unwrap_err();
        assert!(e.contains("speech"), "{e}");
        let speech = r#""speech": {"url": "https://x/v1", "stt_model": "w", "tts_model": "t", "voice": "v"}"#;
        let c = Config::parse(&format!("{{{dev}, {speech}}}")).unwrap();
        assert_eq!(c.esphome.devices[0].host_port(), "h:6053");
    }

    // REQ-VOI-021
    #[test]
    fn the_speech_provider_is_checked_and_brings_its_own_defaults() {
        use crate::speech::Dialect;
        // Unchanged: the OpenAI style still needs its address, models and voice.
        let e =
            Config::parse(r#"{"speech": {"provider": "openai", "stt_model": "w"}}"#).unwrap_err();
        assert!(e.contains("speech.url"), "{e}");
        let e = Config::parse(r#"{"speech": {"provider": "acme"}}"#).unwrap_err();
        assert!(
            e.contains("speech.provider") && e.contains("elevenlabs"),
            "{e}"
        );
        // The others need nothing else.
        let mut c = Config::parse(r#"{"speech": {"provider": "elevenlabs"}}"#).unwrap();
        assert_eq!(c.speech.dialect(), Dialect::ElevenLabs);
        c.speech.apply_provider_defaults();
        assert_eq!(c.speech.url.as_deref(), Some("https://api.elevenlabs.io"));
        assert_eq!(c.speech.key_env.as_deref(), Some("ELEVENLABS_API_KEY"));
        assert!(
            c.speech.voice.is_some()
                && c.speech.tts_model.is_some()
                && c.speech.stt_model.is_some()
        );
        // What the person set is kept.
        let mut c = Config::parse(
            r#"{"speech": {"provider": "deepgram", "voice": "aura-2-luna-en", "key_env": "MY_DG"}}"#,
        )
        .unwrap();
        c.speech.apply_provider_defaults();
        assert_eq!(c.speech.voice.as_deref(), Some("aura-2-luna-en"));
        assert_eq!(c.speech.key_env.as_deref(), Some("MY_DG"));
        assert_eq!(c.speech.stt_model.as_deref(), Some("nova-3"));
        // Not set: nothing is filled in for the OpenAI style.
        let mut c = Config::parse("{}").unwrap();
        c.speech.apply_provider_defaults();
        assert!(c.speech.url.is_none() && c.speech.voice.is_none());
    }

    #[test]
    fn a_misspelt_key_is_an_error_not_ignored() {
        let e = Config::parse(r#"{"listn": "127.0.0.1:1"}"#).unwrap_err();
        assert!(e.contains("listn"), "{e}");
        let e = Config::parse(r#"{"llm": {"modle": "x"}}"#).unwrap_err();
        assert!(e.contains("modle"), "{e}");
    }

    #[test]
    fn bad_values_say_what_is_wrong() {
        for (text, needle) in [
            (r#"{"listen": "nowhere"}"#, "listen"),
            (r#"{"llm": {"url": "https://x"}}"#, "together"),
            (r#"{"llm": {"model": "m"}}"#, "together"),
            (r#"{"llm": {"url": "ftp://x", "model": "m"}}"#, "http"),
            (r#"{"llm": {"summary_model": "m"}}"#, "need"),
            (r#"{"limits": {"daily_spend_usd": -1}}"#, "zero or more"),
            (r#"{"satellite": {"listen": "127.0.0.1:1"}}"#, "speech"),
            (r#"{"satellite": {"listen": "x"}}"#, "address"),
            (r#"{"satellite": {"token_env": "T"}}"#, "satellite.listen"),
            (
                r#"{"satellite": {"listen": "0.0.0.0:10400"}, "speech": {"url": "https://x/v1", "stt_model": "w", "tts_model": "t", "voice": "v"}}"#,
                "token_env",
            ),
            (r#"{"speech": {"stt_model": "w"}}"#, "speech.url"),
            (
                r#"{"speech": {"url": "https://x/v1", "stt_model": "w", "tts_model": "t"}}"#,
                "speech.voice",
            ),
            (
                r#"{"speech": {"url": "https://x/v1", "stt_model": "w", "tts_model": "t", "voice": "v", "tts_sample_rate": 100}}"#,
                "sample_rate",
            ),
            (r#"{"endpoints": [" "]}"#, "empty"),
            (r#"{"name": ""}"#, "`name`"),
            (r#"{"device": {"board": "Echo Dot"}}"#, "device.board"),
            (r#"{"device": {"board": ""}}"#, "device.board"),
            (r#"{"device": {"label": " "}}"#, "device.label"),
            (r#"{"device": {"label": "a\nb"}}"#, "device.label"),
            ("{", "config"),
        ] {
            let e = Config::parse(text).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
        }
    }

    // REQ-RTV-008
    #[test]
    fn the_realtime_section_is_read_and_checked_with_plain_messages() {
        let ok = r#"{"voice": {"realtime": {"provider": "openai", "url": "wss://api.openai.com/v1/realtime",
            "model": "gpt-realtime", "voice": "verse", "key_env": "MY_RT_KEY", "transcription_model": "",
            "usd_per_min_in": 0.05, "usd_per_min_out": 0.2, "max_session_secs": 120}}}"#;
        let c = Config::parse(ok).unwrap();
        let r = &c.voice.realtime;
        assert_eq!(r.model.as_deref(), Some("gpt-realtime"));
        assert_eq!((r.max_session_secs, r.enabled), (Some(120), None));
        assert_eq!(
            Config::parse("{}").unwrap().voice.realtime,
            RealtimeSection::default()
        );
        for (section, needle) in [
            (r#""provider": "other""#, "provider"),
            (r#""url": "wss://x/v1""#, "together"),
            (r#""model": "m""#, "together"),
            (r#""url": "https://x/v1", "model": "m""#, "wss://"),
            (
                r#""url": "wss://x/v1", "model": "m", "key_env": "sk-abc def""#,
                "NAME",
            ),
            (r#""usd_per_min_in": -1"#, "usd_per_min_in"),
            (r#""max_session_secs": 5"#, "max_session_secs"),
            (r#""enabled": true"#, "needs"),
            (
                // REQ-SEC-001: refused at load, not when the session connects.
                r#""url": "ws://api.openai.com/v1/realtime", "model": "m""#,
                "must not cross the internet unencrypted",
            ),
            (r#""keey_env": "X""#, "keey_env"),
        ] {
            let e = Config::parse(&format!(r#"{{"voice": {{"realtime": {{{section}}}}}}}"#))
                .unwrap_err();
            assert!(e.contains(needle), "{section}: {e}");
        }
        let lan = r#"{"voice": {"realtime": {"url": "ws://192.168.1.9:8000/v1", "model": "m"}}}"#;
        assert!(Config::parse(lan).is_ok());
        // Enabling needs a build with the feature; the message says so rather than failing silently.
        let on = r#"{"voice": {"realtime": {"enabled": true, "url": "wss://x/v1", "model": "m"}}}"#;
        if cfg!(feature = "realtime") {
            assert!(Config::parse(on).is_ok());
        } else {
            assert!(
                Config::parse(on)
                    .unwrap_err()
                    .contains("`realtime` feature")
            );
        }
    }

    // REQ-AST-019, REQ-AST-023
    #[test]
    fn the_agent_section_is_read_and_a_zero_limit_is_refused() {
        let c = Config::parse(
            r#"{"agent": {"max_rounds": 4, "max_tool_calls": 6, "deadline_secs": 30,
                "tool_timeout_secs": 10, "plan_speech": false, "progress_speech": false,
                "summarize_over_chars": 0, "notes": true}}"#,
        )
        .unwrap();
        assert_eq!(c.agent.max_rounds, Some(4));
        assert_eq!(c.agent.deadline_secs, Some(30));
        assert_eq!(c.agent.plan_speech, Some(false));
        assert_eq!(c.agent.notes, Some(true));
        assert_eq!(Config::parse("{}").unwrap().agent, AgentConfig::default());
        for bad in [
            "max_rounds",
            "max_tool_calls",
            "deadline_secs",
            "tool_timeout_secs",
        ] {
            let e = Config::parse(&format!(r#"{{"agent": {{"{bad}": 0}}}}"#)).unwrap_err();
            assert!(e.contains(bad) && e.contains("at least 1"), "{e}");
        }
        // a typo is an error, not a silently ignored setting
        assert!(Config::parse(r#"{"agent": {"max_round": 4}}"#).is_err());
    }

    // REQ-CFG-083
    #[test]
    fn the_device_section_is_read_and_checked() {
        let c = Config::parse(
            r#"{"device": {"board": "my-board_2", "label": "Kitchen speaker", "parts": {"leds": true, "wifi": false}}}"#,
        )
        .unwrap();
        assert_eq!(c.device.board.as_deref(), Some("my-board_2"));
        assert_eq!(c.device.label.as_deref(), Some("Kitchen speaker"));
        assert_eq!(
            (c.device.parts.leds, c.device.parts.wifi),
            (Some(true), Some(false))
        );
        assert_eq!(c.device.parts.screen, None);
        assert_eq!(Config::parse("{}").unwrap().device, DeviceConfig::default());
        let long = "a".repeat(61);
        for bad in [
            r#"{"device": {"parts": {"led": true}}}"#.to_owned(),
            r#"{"device": {"parts": {"leds": "yes"}}}"#.to_owned(),
            r#"{"device": {"brand": "x"}}"#.to_owned(),
            format!(r#"{{"device": {{"label": "{long}"}}}}"#),
        ] {
            assert!(Config::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn req_voi_025_listening_is_read_checked_and_defaults_to_the_endpointer_defaults() {
        let c = Config::parse(
            r#"{"satellite": {"listening": {"min_level": 150, "silence_ms": 900, "start_frames": 2}}}"#,
        )
        .unwrap();
        let v = c.satellite.listening.apply(crate::vad::Config::default());
        assert!((v.min_level - 150.0).abs() < f32::EPSILON);
        assert_eq!((v.silence_ms, v.start_frames), (900, 2));
        assert_eq!(v.preroll_ms, crate::vad::Config::default().preroll_ms);
        // Nothing set: today's defaults.
        let none = Config::parse("{}").unwrap();
        assert_eq!(
            none.satellite
                .listening
                .apply(crate::vad::Config::default()),
            crate::vad::Config::default()
        );
        for (text, needle) in [
            (r#"{"min_level": 0}"#, "min_level"),
            (r#"{"min_level": 100000}"#, "min_level"),
            (r#"{"noise_ratio": 0.5}"#, "noise_ratio"),
            (r#"{"start_frames": 0}"#, "start_frames"),
            (r#"{"silence_ms": 10}"#, "silence_ms"),
            (r#"{"min_speech_ms": 5000}"#, "min_speech_ms"),
            (r#"{"max_utterance_ms": 100}"#, "max_utterance_ms"),
            (r#"{"no_speech_ms": 999999}"#, "no_speech_ms"),
            (r#"{"preroll_ms": 5000}"#, "preroll_ms"),
            (r#"{"silence": 5}"#, "silence"),
            (
                r#"{"min_speech_ms": 2000, "max_utterance_ms": 1500}"#,
                "less than",
            ),
        ] {
            let e =
                Config::parse(&format!(r#"{{"satellite": {{"listening": {text}}}}}"#)).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
        }
    }

    #[cfg(feature = "esphome-satellite")]
    #[test]
    fn req_voi_025_a_device_value_is_checked_against_the_satellite_section() {
        let speech = r#""speech": {"url": "https://x/v1", "stt_model": "w", "tts_model": "t", "voice": "v"}"#;
        let ok = format!(
            r#"{{{speech}, "satellite": {{"listening": {{"max_utterance_ms": 2000}}}},
                "esphome": {{"devices": [{{"name": "a", "address": "h", "listening": {{"min_speech_ms": 500}}}}]}}}}"#
        );
        assert!(Config::parse(&ok).is_ok());
        let bad = format!(
            r#"{{{speech}, "satellite": {{"listening": {{"max_utterance_ms": 2000}}}},
                "esphome": {{"devices": [{{"name": "a", "address": "h", "listening": {{"min_speech_ms": 2000}}}}]}}}}"#
        );
        let e = Config::parse(&bad).unwrap_err();
        assert!(e.contains("esphome.devices[a].listening"), "{e}");
    }
}
