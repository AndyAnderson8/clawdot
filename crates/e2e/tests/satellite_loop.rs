//! A satellite holds a conversation with the core, end to end, over real sockets:
//! the reference satellite (std only) against the real satellite server, with fake speech providers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::assistant::Effect;
use clawdot_core::inbox::endpoint::hash_token;
use clawdot_core::satellite::{Answer, Ask, Settings, serve};
use clawdot_core::voice::{Synthesizer, Transcriber};
use clawdot_protocol::message::Indicator;
use clawdot_satellite::{Config, FileDevice, run};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use tokio::net::TcpListener;
use tokio::sync::{mpsc as tmpsc, oneshot};

struct Ears;
impl Transcriber for Ears {
    async fn transcribe(&self, pcm: &[i16]) -> Result<String, String> {
        assert!(
            pcm.len() > 8000,
            "a real utterance arrives, not a fragment: {}",
            pcm.len()
        );
        Ok("what is on my plate".to_owned())
    }
}

struct Mouth;
impl Synthesizer for Mouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        Ok(vec![9; text.len() * 50])
    }
}

/// Run the core's satellite server on its own thread; returns its address and a stop switch.
fn start_core(token: Option<&str>) -> (SocketAddr, oneshot::Sender<()>, mpsc::Receiver<String>) {
    start_core_with(token, |_| Answer {
        say: "Nothing is waiting.".to_owned(),
        effects: vec![Effect::VolumeSet(40)],
    })
}

/// Like `start_core`, with a custom answer for each question.
fn start_core_with(
    token: Option<&str>,
    respond: fn(&str) -> Answer,
) -> (SocketAddr, oneshot::Sender<()>, mpsc::Receiver<String>) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (asked_tx, asked_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let settings = Settings {
        token_hashes: token.map(|t| vec![hash_token(t)]).unwrap_or_default(),
        ..Settings::default()
    };
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            let (asks, mut rx) = tmpsc::channel::<Ask>(4);
            tokio::spawn(async move {
                while let Some(ask) = rx.recv().await {
                    asked_tx.send(ask.text.clone()).unwrap();
                    let _ = ask.reply.send(respond(&ask.text));
                }
            });
            let _ = serve(
                listener,
                Arc::new(Ears),
                Arc::new(Mouth),
                asks,
                settings,
                async {
                    let _ = stop_rx.await;
                },
            )
            .await;
        });
    });
    (addr_rx.recv().unwrap(), stop_tx, asked_rx)
}

fn utterance() -> Vec<i16> {
    let mut v = vec![0i16; 3200];
    v.extend((0..9600).map(|i| (3000.0 * (f64::from(i) * 0.35).sin()) as i16));
    v
}

fn cfg(token: Option<&str>) -> Config {
    Config {
        device_id: "e2e".into(),
        token: token.map(str::to_owned),
        wake_on_connect: true,
        dead_after: clawdot_satellite::DEAD_AFTER,
    }
}

#[test]
fn a_reference_satellite_gets_a_spoken_answer_from_the_core() {
    let (addr, stop, asked) = start_core(None);
    let mut dev = FileDevice::new(utterance());
    let report = run(TcpStream::connect(addr).unwrap(), &mut dev, &cfg(None)).unwrap();

    assert_eq!(
        asked.recv().unwrap(),
        "what is on my plate",
        "the transcript reaches the assistant"
    );
    assert!(report.indicators.contains(&Indicator::Listening));
    assert!(report.indicators.contains(&Indicator::Thinking));
    assert!(report.indicators.contains(&Indicator::Speaking));
    assert_eq!(
        dev.reply.len(),
        "Nothing is waiting.".len() * 50,
        "the reply audio arrives intact"
    );
    assert!(dev.reply.iter().all(|&s| s == 9));
    assert_eq!(dev.volume, Some(40), "a volume effect reaches the device");
    let _ = stop.send(());
}

#[test]
fn the_token_is_checked_and_a_refusal_says_why() {
    let (addr, stop, _asked) = start_core(Some("sat-token"));
    let e = run(
        TcpStream::connect(addr).unwrap(),
        &mut FileDevice::new(utterance()),
        &cfg(None),
    )
    .unwrap_err();
    assert!(e.contains("unauthorized"), "{e}");
    let e = run(
        TcpStream::connect(addr).unwrap(),
        &mut FileDevice::new(utterance()),
        &cfg(Some("wrong")),
    )
    .unwrap_err();
    assert!(e.contains("unauthorized"), "{e}");
    let mut dev = FileDevice::new(utterance());
    run(
        TcpStream::connect(addr).unwrap(),
        &mut dev,
        &cfg(Some("sat-token")),
    )
    .unwrap();
    assert!(!dev.reply.is_empty(), "the right token gets an answer");
    let _ = stop.send(());
}

#[test]
fn radio_reaches_a_satellite_that_can_play_streams() {
    let (addr, stop, _asked) = start_core_with(None, |_| Answer {
        say: "Playing jazz.".to_owned(),
        effects: vec![Effect::PlayStream {
            url: "http://radio.example/jazz".into(),
            title: "jazz".into(),
        }],
    });
    let mut dev = FileDevice::new(utterance());
    dev.can_stream = true;
    dev.wait_for_streams = 1;
    run(TcpStream::connect(addr).unwrap(), &mut dev, &cfg(None)).unwrap();
    assert_eq!(
        dev.streams,
        vec![("http://radio.example/jazz".to_owned(), "jazz".to_owned())]
    );
    assert_eq!(
        dev.reply.len(),
        "Playing jazz.".len() * 50,
        "the confirmation was spoken first"
    );
    let _ = stop.send(());
}
