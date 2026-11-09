//! A spoken turn through the audio interface against the real satellite server, with fake speech providers
//! and no audio hardware: REQ-VOI-013..018. The microphone is a recording or a scripted child process that
//! prints one, the speaker is a recording or a child process that stores what it is given.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::assistant::Effect;
use clawdot_core::satellite::{Answer, Ask, Settings, serve};
use clawdot_core::voice::{Synthesizer, Transcriber};
use clawdot_satellite::audio::{WavIn, WavOut};
use clawdot_satellite::front::{AudioDevice, DeviceConfig, Immediate, Press, Scripted};
use clawdot_satellite::{Config, run};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use tokio::net::TcpListener;
use tokio::sync::{mpsc as tmpsc, oneshot};

struct Ears;
impl Transcriber for Ears {
    async fn transcribe(&self, pcm: &[i16]) -> Result<String, String> {
        assert!(pcm.len() > 8000, "a real utterance arrives: {}", pcm.len());
        Ok("what is on my plate".to_owned())
    }
}

struct Mouth;
impl Synthesizer for Mouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        Ok(vec![9; text.len() * 50])
    }
}

fn start_core() -> (SocketAddr, oneshot::Sender<()>, mpsc::Receiver<String>) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (asked_tx, asked_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
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
                    let _ = ask.reply.send(Answer {
                        say: "Nothing is waiting.".to_owned(),
                        effects: vec![Effect::VolumeSet(40)],
                    });
                }
            });
            let _ = serve(
                listener,
                Arc::new(Ears),
                Arc::new(Mouth),
                asks,
                Settings::default(),
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

fn cfg() -> Config {
    Config {
        device_id: "e2e-audio".into(),
        token: None,
        wake_on_connect: false,
        dead_after: clawdot_satellite::DEAD_AFTER,
    }
}

const REPLY_SAMPLES: usize = "Nothing is waiting.".len() * 50;

#[test]
fn req_voi_016_a_turn_runs_from_a_recording_to_a_recording_through_the_real_core() {
    let (addr, stop, asked) = start_core();
    let mut dev = AudioDevice::new(
        WavIn::new(utterance(), 1_500),
        WavOut::in_memory(),
        Immediate::default(),
        DeviceConfig {
            max_turns: Some(1),
            ..DeviceConfig::default()
        },
    );
    run(TcpStream::connect(addr).unwrap(), &mut dev, &cfg()).unwrap();
    assert_eq!(asked.recv().unwrap(), "what is on my plate");
    assert_eq!(dev.output().samples.len(), REPLY_SAMPLES);
    assert!(dev.output().samples.iter().all(|&s| s == 9));
    assert!(dev.problems.is_empty(), "{:?}", dev.problems);
    let _ = stop.send(());
}

#[test]
fn req_voi_017_a_press_with_a_scripted_microphone_and_player_process_runs_a_turn() {
    if !cfg!(unix) {
        return;
    }
    use clawdot_satellite::command::{CommandIn, CommandOut, custom};
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!("clawdot-e2e-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mic_file, spk_file) = (dir.join("mic.raw"), dir.join("spk.raw"));
    let mut pcm = utterance();
    pcm.extend(std::iter::repeat_n(0i16, 24_000));
    let frames = pcm.len() / 320;
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    std::fs::write(&mic_file, bytes).unwrap();

    let (addr, stop, asked) = start_core();
    // Generous: the stall clock starts at spawn, and these children can start slowly on a loaded machine.
    let stall = Duration::from_secs(120);
    let mut dev = AudioDevice::new(
        CommandIn::new(
            // A paced fake microphone: one 20 ms frame every 10 ms, like a live source (a burst would
            // overflow the one-second buffer, which drops the oldest audio by design).
            custom(
                "sh",
                &[
                    "-c".into(),
                    format!(
                        "i=0; while [ $i -lt {frames} ]; do dd if='{}' bs=640 skip=$i count=1 2>/dev/null; sleep 0.01; i=$((i+1)); done",
                        mic_file.to_string_lossy()
                    ),
                ],
                false,
            ),
            stall,
        ),
        CommandOut::new(
            custom(
                "sh",
                &[
                    "-c".into(),
                    format!("cat > '{}'", spk_file.to_string_lossy()),
                ],
                false,
            ),
            stall,
        ),
        Scripted([Press::Talk].into()),
        DeviceConfig {
            max_turns: Some(1),
            ..DeviceConfig::default()
        },
    );
    // Run on a thread so a stuck turn fails the test instead of hanging it.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let r = run(TcpStream::connect(addr).unwrap(), &mut dev, &cfg());
        let _ = tx.send((r, dev));
    });
    let (r, dev) = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the turn did not finish");
    r.unwrap();
    assert_eq!(asked.recv().unwrap(), "what is on my plate");
    assert!(dev.problems.is_empty(), "{:?}", dev.problems);
    let played: Vec<i16> = std::fs::read(&spk_file)
        .unwrap()
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    assert_eq!(
        played.len(),
        REPLY_SAMPLES,
        "the reply reached the player intact"
    );
    // The reply set the volume to 40, which the command backend applies: 9 * 40 / 100.
    assert!(played.iter().all(|&s| s == 3), "{:?}", &played[..8]);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = stop.send(());
}
