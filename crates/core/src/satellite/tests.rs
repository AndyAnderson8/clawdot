use super::*;
use crate::assistant::{Effect, MediaCmd};
use crate::inbox::endpoint::hash_token;
use crate::vad::tests::{silence, tone};
use crate::voice::{Synthesizer, Transcriber};
use clawdot_protocol::frame::{Decoder, Limits, encode};
use clawdot_protocol::message::{
    AudioCodec, AudioFormat, Indicator, MediaAction, Message, PROTOCOL_VERSION,
};
use clawdot_protocol::message::{Capabilities, Hello, Version, Wake};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::DuplexStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::time::timeout;

struct Ears(&'static str);
impl Transcriber for Ears {
    async fn transcribe(&self, _: &[i16]) -> Result<String, String> {
        Ok(self.0.to_owned())
    }
}
struct Mouth;
impl Synthesizer for Mouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        Ok(vec![7; text.len() * 100])
    }
}

/// The satellite end of the pipe.
struct Peer {
    io: DuplexStream,
    dec: Decoder,
}

impl Peer {
    async fn send(&mut self, m: &Message) {
        self.io
            .write_all(&encode(&m.to_frame()).unwrap())
            .await
            .unwrap();
    }
    async fn recv(&mut self) -> Message {
        timeout(
            Duration::from_secs(5),
            next_message(&mut self.io, &mut self.dec),
        )
        .await
        .expect("timed out waiting for the core")
        .unwrap()
        .expect("the core closed the connection")
    }
    /// True if nothing but keepalives arrives for `ms` milliseconds.
    async fn quiet_for(&mut self, ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            match timeout(left, next_message(&mut self.io, &mut self.dec)).await {
                Err(_) => return true,
                Ok(Ok(Some(Message::Ping))) => self.send(&Message::Pong).await,
                Ok(_) => return false,
            }
        }
        true
    }
    async fn hello(&mut self, token: Option<&str>, protocol: Version) {
        self.send(&Message::Hello(Hello {
            protocol,
            device_id: "test-dot".into(),
            name: None,
            token: token.map(str::to_owned),
            capabilities: Capabilities {
                wake_word: true,
                ..Capabilities::default()
            },
        }))
        .await;
    }
    async fn say(&mut self, ms: usize) {
        let mut audio = silence(200);
        audio.extend(tone(3000.0, ms));
        audio.extend(silence(1000));
        for chunk in audio.chunks(320) {
            let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            self.send(&Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            })
            .await;
        }
    }
}

/// Start a session; the "assistant" answers every question with `answer`.
fn start(
    settings: Settings,
    heard: &'static str,
    answer: &'static str,
) -> (Peer, tokio::task::JoinHandle<Result<(), String>>) {
    let (near, far) = tokio::io::duplex(1 << 20);
    let (tx, mut rx) = mpsc::channel::<Ask>(4);
    tokio::spawn(async move {
        while let Some(ask) = rx.recv().await {
            let _ = ask.reply.send(Answer {
                say: format!("{answer} ({})", ask.text),
                effects: vec![Effect::VolumeSet(30)],
                ..Answer::default()
            });
        }
    });
    let h = tokio::spawn(run_session(
        far,
        Arc::new(Ears(heard)),
        Arc::new(Mouth),
        tx,
        settings,
    ));
    (
        Peer {
            io: near,
            dec: Decoder::new(Limits::default()),
        },
        h,
    )
}

fn fast() -> Settings {
    Settings {
        keepalive: Duration::from_millis(50),
        dead_after: Duration::from_millis(400),
        hello_timeout: Duration::from_millis(300),
        ..Settings::default()
    }
}

// ----- music on a satellite (REQ-MUS-001, REQ-MUS-004) -----

/// Wake the satellite and speak one utterance.
async fn speak(p: &mut Peer) {
    p.send(&Message::Wake(Wake {
        name: "hey".into(),
        timestamp_ms: None,
    }))
    .await;
    assert_eq!(p.recv().await, Message::SetIndicator(Indicator::Listening));
    assert_eq!(p.recv().await, Message::StreamStart);
    p.say(600).await;
}

/// Messages (without keepalives) up to and including the first one `done` accepts.
async fn until(p: &mut Peer, done: impl Fn(&Message) -> bool) -> Vec<Message> {
    let mut seen = Vec::new();
    loop {
        match p.recv().await {
            Message::Ping => p.send(&Message::Pong).await,
            m => {
                let stop = done(&m);
                seen.push(m);
                if stop {
                    return seen;
                }
            }
        }
    }
}

fn play_jazz(_: &str) -> Answer {
    Answer {
        say: "Playing jazz.".into(),
        effects: vec![Effect::PlayStream {
            url: "http://r.example/jazz".into(),
            title: "jazz".into(),
        }],
        ..Answer::default()
    }
}

// ----- a turn that takes a while: speech while working and stopping it (REQ-AST-020, 022, VOI-020) -----

/// Answers the questions it is asked with the given texts, one per call.
struct Cycle(Mutex<VecDeque<&'static str>>);
impl Cycle {
    fn new(texts: &[&'static str]) -> Self {
        Self(Mutex::new(texts.iter().copied().collect()))
    }
}
impl Transcriber for Cycle {
    async fn transcribe(&self, _: &[i16]) -> Result<String, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or("")
            .to_owned())
    }
}

fn samples(seen: &[Message]) -> usize {
    seen.iter()
        .filter_map(|m| match m {
            Message::AudioChunk { payload, .. } => Some(payload.len() / 2),
            _ => None,
        })
        .sum()
}

fn wake() -> Message {
    Message::Wake(Wake {
        name: "hey".into(),
        timestamp_ms: None,
    })
}

/// Say something and wait until the core is thinking about it.
async fn ask_and_wait_for_thinking(p: &mut Peer) {
    speak(p).await;
    until(p, |m| *m == Message::SetIndicator(Indicator::Thinking)).await;
}

const STOPPED: &str = "Okay, I stopped.";

// ----- the administrator reset request (REQ-SAT-020, REQ-DEV-031) -----

// ----- slow speech synthesis never stalls the session (REQ-SAT-008, REQ-VOI-020) -----

/// Everything the core sends for `ms` milliseconds, with its pings answered.
async fn collect_for(p: &mut Peer, ms: u64) -> Vec<Message> {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut seen = Vec::new();
    while Instant::now() < end {
        let left = end.saturating_duration_since(Instant::now());
        match timeout(left, next_message(&mut p.io, &mut p.dec)).await {
            Err(_) => break,
            Ok(Ok(Some(Message::Ping))) => p.send(&Message::Pong).await,
            Ok(Ok(Some(m))) => seen.push(m),
            Ok(_) => break,
        }
    }
    seen
}

// ----- a stream command never outlives a reply that could not be spoken (REQ-MUS-004) -----

// ----- a satellite that stops reading cannot wedge the session (REQ-SAT-010) -----

// ----- a preview voice travels with the request (REQ-CFG-080) -----

mod admin_reset;
mod announce;
mod music;
mod reply;
mod session;
mod slow_speech;
mod turn;
