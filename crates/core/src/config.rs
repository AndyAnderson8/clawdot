//! The single human-editable configuration file (REQ-CFG-007).
//!
//! JSON, so the core needs no extra parser. Every key is optional; command-line flags override the file.
//! Unknown keys are errors, so a typo is reported at startup instead of silently ignored. Secrets are never
//! stored here: the provider key comes from an environment variable named by `llm.key_env`.

use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Language-model settings.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    /// OpenAI-compatible base URL, for example `https://openrouter.ai/api/v1`.
    pub url: Option<String>,
    /// Model used for conversation and tool calls.
    pub model: Option<String>,
    /// Model to try when the main one fails.
    pub fallback_model: Option<String>,
    /// Cheaper model for spoken summaries.
    pub summary_model: Option<String>,
    /// Name of the environment variable that holds the API key.
    pub key_env: Option<String>,
}

/// Usage caps.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimitsConfig {
    /// Requests per local day; must be at least 1.
    pub max_requests_per_day: Option<u32>,
    /// Spend per local day in US dollars; must not be negative.
    pub daily_spend_usd: Option<f64>,
}

/// Speech-to-text and text-to-speech provider (OpenAI-compatible).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpeechConfig {
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
    /// Speech provider.
    #[serde(default)]
    pub speech: SpeechConfig,
    /// Radio stations for the music commands.
    #[serde(default)]
    pub stations: Vec<crate::media::Station>,
    /// Declarative HTTP tools the model may call (see `docs/architecture/http-tools.md`).
    #[serde(default)]
    pub tools: Vec<crate::httptools::ToolDef>,
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
        self.listen_addr()?;
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
        if self.limits.max_requests_per_day == Some(0) {
            return Err("config: `limits.max_requests_per_day` must be at least 1".into());
        }
        if let Some(s) = self.limits.daily_spend_usd
            && (!s.is_finite() || s < 0.0)
        {
            return Err("config: `limits.daily_spend_usd` must be zero or more".into());
        }
        let sp = &self.speech;
        if sp.url.is_some() {
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
        for s in &self.stations {
            s.validate().map_err(|e| format!("config: {e}"))?;
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
              "public_url": "http://dot.local:8080", "persona_file": "persona.txt",
              "endpoints": ["claude-code", "ci:generic"],
              "llm": {"url": "https://openrouter.ai/api/v1", "model": "a/b", "fallback_model": "c/d",
                      "summary_model": "e/f", "key_env": "MY_KEY"},
              "limits": {"max_requests_per_day": 200, "daily_spend_usd": 1.5}
            }"#,
        )
        .unwrap();
        assert_eq!(c.listen_addr().unwrap().unwrap().port(), 8080);
        assert_eq!(c.endpoints.len(), 2);
        assert_eq!(c.limits.max_requests_per_day, Some(200));
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
            (r#"{"limits": {"max_requests_per_day": 0}}"#, "at least 1"),
            (r#"{"limits": {"daily_spend_usd": -1}}"#, "zero or more"),
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
            ("{", "config"),
        ] {
            let e = Config::parse(text).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
        }
    }
}
