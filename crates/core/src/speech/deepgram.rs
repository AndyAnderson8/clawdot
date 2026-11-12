//! Deepgram: `Authorization: Token`, `POST /v1/listen` (Nova) and `POST /v1/speak` (Aura). It has no list of
//! voices to ask for, so the Aura 2 English voices are kept here (REQ-VOI-021).

use super::{Defaults, Provider, Speak, VoiceInfo, VoiceList, plain_id};
use crate::apiclient::{Call, Wire, error_field, keyed_service_reason, without_v1};
use serde_json::{Value, json};

/// Deepgram has no list to ask for; these are its Aura 2 English voices (the model name is the voice).
const VOICES: [(&str, &str); 12] = [
    ("aura-2-thalia-en", "Thalia"),
    ("aura-2-andromeda-en", "Andromeda"),
    ("aura-2-helena-en", "Helena"),
    ("aura-2-luna-en", "Luna"),
    ("aura-2-athena-en", "Athena"),
    ("aura-2-aurora-en", "Aurora"),
    ("aura-2-apollo-en", "Apollo"),
    ("aura-2-arcas-en", "Arcas"),
    ("aura-2-orion-en", "Orion"),
    ("aura-2-atlas-en", "Atlas"),
    ("aura-2-zeus-en", "Zeus"),
    ("aura-2-hermes-en", "Hermes"),
];

/// The Deepgram API.
#[derive(Debug, Clone, Copy, Default)]
pub struct Deepgram;

impl Wire for Deepgram {
    fn title(&self) -> &'static str {
        "Deepgram"
    }

    fn needs_key(&self) -> bool {
        true
    }

    fn auth_header(&self, key: &str) -> (&'static str, String) {
        ("authorization", format!("Token {key}"))
    }

    // The paths here start with /v1 themselves; an address that already ends in it still works.
    fn trim_base<'a>(&self, base: &'a str) -> &'a str {
        without_v1(base)
    }

    // {"error":{"message"}}, {"err_msg"} or {"message"}.
    fn error_message(&self, body: &Value) -> String {
        error_field(body)
            .or_else(|| {
                [&body["err_msg"], &body["message"]]
                    .iter()
                    .find_map(|x| x.as_str())
            })
            .unwrap_or_default()
            .to_owned()
    }

    fn failure_reason(&self, code: u16) -> &'static str {
        keyed_service_reason(code)
    }
}

impl Provider for Deepgram {
    fn name(&self) -> &'static str {
        "deepgram"
    }

    fn wire(&self) -> &'static dyn Wire {
        &Deepgram
    }

    fn defaults(&self) -> Defaults {
        Defaults {
            url: Some("https://api.deepgram.com"),
            stt_model: Some("nova-3"),
            // Deepgram picks its model by voice, so this is only a placeholder.
            tts_model: Some("aura-2"),
            voice: Some("aura-2-thalia-en"),
            key_env: Some("DEEPGRAM_API_KEY"),
        }
    }

    fn pcm_rates(&self) -> &'static [u32] {
        &[8_000, 16_000, 24_000, 32_000, 48_000]
    }

    fn transcription(
        &self,
        model: &str,
        language: Option<&str>,
        pcm: &[i16],
    ) -> Result<Call, String> {
        plain_id("The transcription model", model)?;
        let lang = match language {
            Some(l) => {
                plain_id("The language", l)?;
                format!("language={l}")
            }
            None => "detect_language=true".to_owned(),
        };
        Ok(Call::post(
            format!("/v1/listen?model={model}&smart_format=true&{lang}"),
            "audio/wav",
            crate::audio::wav(pcm),
        ))
    }

    fn transcript<'a>(&self, reply: &'a Value) -> Option<&'a str> {
        reply["results"]["channels"][0]["alternatives"][0]["transcript"].as_str()
    }

    fn synthesis(&self, speak: &Speak<'_>) -> Result<Call, String> {
        // The voice is the model: `aura-2-thalia-en`.
        plain_id("The voice", speak.voice)?;
        let body = json!({ "text": speak.text });
        Ok(Call::post(
            format!(
                "/v1/speak?model={}&encoding=linear16&container=none&sample_rate={}",
                speak.voice, speak.rate
            ),
            "application/json",
            body.to_string().into_bytes(),
        ))
    }

    fn voices<'a>(&'a self, _base_url: &'a str, _key: &'a str) -> VoiceList<'a> {
        let voices = VOICES
            .iter()
            .map(|(id, label)| VoiceInfo {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
                category: None,
            })
            .collect();
        Box::pin(std::future::ready(Ok((voices, "builtin"))))
    }
}
