//! Optional ESPHome voice-satellite client (spec 0004), behind the `esphome-satellite` cargo feature.
//!
//! The core dials an ESPHome voice satellite (a Dot running a base image, a Voice PE, linux-voice-assistant)
//! and uses it as a microphone and speaker, with no Home Assistant in between. Layers, bottom to top:
//!
//! * [`proto`]: a hand-written minimal protobuf codec for the messages used;
//! * [`frame`]: plaintext framing and the [`frame::Link`] that reads and writes messages;
//! * `noise` (feature `esphome-noise`): the encrypted variant of the framing;
//! * [`media`]: a tiny file server for audio the device fetches by URL;
//! * [`client`]: setup, and the translation to and from the satellite protocol, so the existing pipeline serves
//!   the device unchanged.

pub mod client;
pub mod frame;
pub mod media;
#[cfg(feature = "esphome-noise")]
pub mod noise;
pub mod proto;

use crate::esphome_cfg::EsphomeConfig;
use crate::satellite::{Ask, Settings};
use crate::voice::{Synthesizer, Transcriber};
use std::sync::Arc;
use tokio::sync::mpsc;

pub use client::{Device, Pipeline, Timing, run_device};

#[cfg(feature = "esphome-noise")]
fn read_psk(var: &str) -> Result<Option<[u8; 32]>, String> {
    let text = std::env::var(var).unwrap_or_default();
    if text.trim().is_empty() {
        return Err(format!(
            "environment variable {var} (esphome psk_env) is not set"
        ));
    }
    noise::decode_psk(text.trim()).map(Some)
}

#[cfg(not(feature = "esphome-noise"))]
fn read_psk(var: &str) -> Result<Option<[u8; 32]>, String> {
    Err(format!(
        "`psk_env` ({var}) needs a build with the `esphome-noise` feature"
    ))
}

/// Start the file server (when configured) and one task per device. Returns once everything is bound; the
/// tasks run for the life of the program.
///
/// # Errors
/// A message when the file server cannot bind or a device's key is missing or malformed.
pub async fn start<S, T>(
    cfg: &EsphomeConfig,
    stt: Arc<S>,
    tts: Arc<T>,
    asks: mpsc::Sender<Ask>,
    settings: Settings,
) -> Result<(), String>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    let mut media_store = None;
    if let (Some(listen), Some(url)) = (&cfg.media_listen, &cfg.media_url) {
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .map_err(|e| format!("esphome.media_listen {listen}: {e}"))?;
        let store = media::MediaStore::new(url);
        tokio::spawn(media::serve(
            listener,
            store.clone(),
            std::future::pending(),
        ));
        println!("clawdot: esphome audio files are served on {listen}");
        media_store = Some(store);
    }
    for d in &cfg.devices {
        let psk = match &d.psk_env {
            Some(var) => read_psk(var)?,
            None => None,
        };
        let device = Device {
            name: d.name.clone(),
            addr: d.host_port(),
            psk,
        };
        println!("clawdot: dialling ESPHome device {}", d.name);
        tokio::spawn(run_device(
            device,
            media_store.clone(),
            Pipeline {
                stt: Arc::clone(&stt),
                tts: Arc::clone(&tts),
                asks: asks.clone(),
                settings: settings.clone(),
            },
            Timing::default(),
            std::future::pending(),
        ));
    }
    Ok(())
}
