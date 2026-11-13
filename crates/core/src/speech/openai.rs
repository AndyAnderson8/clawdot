//! The OpenAI style of speech API: `POST /audio/transcriptions`, `POST /audio/speech` and a bearer key.
//!
//! OpenAI itself, Kokoro, Piper helpers, Whisper servers and most self-hosted speech servers copy it. There is no
//! default address (the address says which server it is) and a key is only sent when there is one.

use super::{Defaults, Provider, Speak, VoiceInfo, VoiceList};
use crate::apiclient::{Bearer, Call, ProviderError, Wire, list_names};
use serde_json::json;

/// The OpenAI-style speech API.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAi;

impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn wire(&self) -> &'static dyn Wire {
        &Bearer
    }

    fn defaults(&self) -> Defaults {
        Defaults::default()
    }

    fn transcription(
        &self,
        model: &str,
        language: Option<&str>,
        pcm: &[i16],
    ) -> Result<Call, String> {
        let mut fields = vec![("model", model), ("response_format", "json")];
        if let Some(l) = language {
            fields.push(("language", l));
        }
        Ok(Call::wav_form("/audio/transcriptions", &fields, pcm))
    }

    fn synthesis(&self, speak: &Speak<'_>) -> Result<Call, String> {
        let body = json!({
            "model": speak.model,
            "voice": speak.voice,
            "input": speak.text,
            "response_format": "pcm",
        });
        Ok(Call::post(
            "/audio/speech",
            "application/json",
            body.to_string().into_bytes(),
        ))
    }

    fn voices<'a>(&'a self, base_url: &'a str, key: &'a str) -> VoiceList<'a> {
        Box::pin(async move {
            let names = list_voices(base_url, key).await?;
            Ok((
                names
                    .into_iter()
                    .map(|n| VoiceInfo {
                        label: n.clone(),
                        id: n,
                        category: None,
                    })
                    .collect(),
                "server",
            ))
        })
    }
}

/// The voices a speech server offers, from `GET <base>/audio/voices` or the other places servers put them (not every server has a list).
///
/// # Errors
/// A plain description of why the list could not be had.
pub async fn list_voices(base_url: &str, key: &str) -> Result<Vec<String>, ProviderError> {
    // Servers that list their voices put the list in a few different places: OpenAI-compatible ones under
    // /audio/voices (Kokoro, the Piper helper), others at /voices or /audio/speech/voices.
    let mut first = None;
    for path in ["/audio/voices", "/voices", "/audio/speech/voices"] {
        match list_names(base_url, key, path, "voices", &Bearer).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                first.get_or_insert(e);
            }
        }
    }
    Err(first.unwrap_or_else(|| ProviderError::Other("no list".to_owned())))
}
