//! ElevenLabs: `xi-api-key`, `POST /v1/speech-to-text` (Scribe), `POST /v1/text-to-speech/{voice_id}` and the voice
//! list (cloned voices included) from `GET /v2/voices` (REQ-VOI-021, REQ-VOI-022).

use super::{Defaults, Provider, Speak, VoiceInfo, VoiceList, plain_id};
use crate::apiclient::{
    Call, Client, ProviderError, Wire, error_field, keyed_service_reason, without_v1,
};
use serde_json::{Value, json};
use std::time::Duration;

/// The ElevenLabs API.
#[derive(Debug, Clone, Copy, Default)]
pub struct ElevenLabs;

impl Wire for ElevenLabs {
    fn title(&self) -> &'static str {
        "ElevenLabs"
    }

    fn needs_key(&self) -> bool {
        true
    }

    fn auth_header(&self, key: &str) -> (&'static str, String) {
        ("xi-api-key", key.to_owned())
    }

    // The paths here start with /v1 themselves; an address that already ends in it still works.
    fn trim_base<'a>(&self, base: &'a str) -> &'a str {
        without_v1(base)
    }

    // {"error":{"message"}}, {"detail":{"status","message"}}, {"detail":"text"} or {"detail":[{"msg"}]}.
    fn error_message(&self, body: &Value) -> String {
        error_field(body)
            .or_else(|| {
                [
                    &body["detail"]["message"],
                    &body["detail"],
                    &body["detail"][0]["msg"],
                    &body["message"],
                ]
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

impl Provider for ElevenLabs {
    fn name(&self) -> &'static str {
        "elevenlabs"
    }

    fn wire(&self) -> &'static dyn Wire {
        &ElevenLabs
    }

    fn defaults(&self) -> Defaults {
        Defaults {
            url: Some("https://api.elevenlabs.io"),
            stt_model: Some("scribe_v2"),
            tts_model: Some("eleven_flash_v2_5"),
            // "Rachel", a premade voice from ElevenLabs' own quick start.
            voice: Some("21m00Tcm4TlvDq8ikWAM"),
            key_env: Some("ELEVENLABS_API_KEY"),
        }
    }

    fn pcm_rates(&self) -> &'static [u32] {
        &[8_000, 16_000, 22_050, 24_000, 44_100]
    }

    fn transcription(
        &self,
        model: &str,
        language: Option<&str>,
        pcm: &[i16],
    ) -> Result<Call, String> {
        let mut fields = vec![("model_id", model)];
        // ElevenLabs takes ISO 639 codes, so `pt-BR` is sent as `pt`.
        let code = language.map(|l| l.split('-').next().unwrap_or(l));
        if let Some(c) = code {
            fields.push(("language_code", c));
        }
        Ok(Call::wav_form("/v1/speech-to-text", &fields, pcm))
    }

    fn synthesis(&self, speak: &Speak<'_>) -> Result<Call, String> {
        plain_id("The voice", speak.voice)?;
        let body = json!({ "text": speak.text, "model_id": speak.model });
        Ok(Call::post(
            format!(
                "/v1/text-to-speech/{}?output_format=pcm_{}",
                speak.voice, speak.rate
            ),
            "application/json",
            body.to_string().into_bytes(),
        ))
    }

    fn voices<'a>(&'a self, base_url: &'a str, key: &'a str) -> VoiceList<'a> {
        Box::pin(async move { Ok((voices(base_url, key).await?, "server")) })
    }
}

/// The voices ElevenLabs lists for this key, cloned ones included, with their names. Pages of 100, at most 10 pages.
async fn voices(base_url: &str, key: &str) -> Result<Vec<VoiceInfo>, ProviderError> {
    let c = Client::new(base_url, key, Duration::from_secs(10), &ElevenLabs)?;
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let mut path = "/v2/voices?page_size=100&include_total_count=false".to_owned();
        if let Some(t) = &token {
            plain_id("The page token", t)?;
            path.push_str("&next_page_token=");
            path.push_str(t);
        }
        let raw = c.send(Call::get(path)).await?;
        let v: Value = serde_json::from_slice(&raw)
            .map_err(|_| ProviderError::from("the list of voices is not readable".to_owned()))?;
        for x in v["voices"].as_array().into_iter().flatten() {
            let (Some(id), name) = (x["voice_id"].as_str(), x["name"].as_str()) else {
                continue;
            };
            if id.is_empty() || id.len() > 128 {
                continue;
            }
            let label: String = name.unwrap_or(id).chars().take(128).collect();
            out.push(VoiceInfo {
                id: id.to_owned(),
                label,
                category: x["category"].as_str().map(|s| s.chars().take(32).collect()),
            });
        }
        token = v["next_page_token"].as_str().map(str::to_owned);
        if v["has_more"].as_bool() != Some(true) || token.is_none() {
            break;
        }
    }
    out.sort_by_key(|a| (a.label.to_lowercase(), a.id.clone()));
    out.dedup_by(|a, b| a.id == b.id);
    out.truncate(1000);
    if out.is_empty() {
        return Err("the service did not list any voices".to_owned().into());
    }
    Ok(out)
}
