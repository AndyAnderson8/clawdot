//! Announcements from elsewhere in the program (REQ-SAT-008) and the preview voice that travels with a request (REQ-CFG-080).

use super::*;

async fn connect_and_register(s: Settings) -> (Peer, Announcer) {
    let ann = s.announcer.clone();
    let (mut p, h) = start(s, "x", "y");
    std::mem::forget(h);
    p.hello(None, PROTOCOL_VERSION).await;
    assert!(matches!(p.recv().await, Message::Welcome(_)));
    for _ in 0..100 {
        if ann.connected() == 1 {
            return (p, ann);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the session never registered");
}

#[tokio::test]
async fn announcements_are_spoken_when_idle_and_open_the_follow_up_window() {
    let s = fast();
    let ann = s.announcer.clone();
    assert_eq!(ann.connected(), 0);
    assert!(
        !ann.announce("nobody is listening").await,
        "no satellite, no announcement"
    );

    let (mut p, ann) = connect_and_register(s).await;
    assert!(ann.is_available().await);
    assert!(ann.announce("Claude finished. Want to reply?").await);

    let mut seen = Vec::new();
    loop {
        match p.recv().await {
            Message::Ping => p.send(&Message::Pong).await,
            m => {
                let last = matches!(m, Message::AudioStop);
                seen.push(m);
                if last {
                    break;
                }
            }
        }
    }
    assert_eq!(seen[0], Message::SetIndicator(Indicator::Speaking));
    let samples: usize = seen
        .iter()
        .filter_map(|m| match m {
            Message::AudioChunk { payload, .. } => Some(payload.len() / 2),
            _ => None,
        })
        .sum();
    assert_eq!(
        samples,
        ("Claude finished.".len() + "Want to reply?".len()) * 100
    );
    assert!(
        !ann.is_available().await,
        "busy while the satellite is playing"
    );

    // Playback ends: the user can answer straight away, without the wake word.
    p.send(&Message::PlaybackDone { id: None }).await;
    loop {
        match p.recv().await {
            Message::Ping => p.send(&Message::Pong).await,
            m => {
                assert_eq!(m, Message::SetIndicator(Indicator::Listening));
                break;
            }
        }
    }
    assert_eq!(p.recv().await, Message::StreamStart);
    assert!(!ann.is_available().await, "listening for the answer");
}

#[tokio::test]
async fn announcements_never_play_into_a_muted_room() {
    let (mut p, ann) = connect_and_register(fast()).await;
    p.send(&Message::MuteChanged { muted: true }).await;
    // Wait until the satellite has processed the mute.
    for _ in 0..100 {
        if !ann.is_available().await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!ann.announce("hello?").await);
    p.send(&Message::MuteChanged { muted: false }).await;
    for _ in 0..100 {
        if ann.is_available().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("unmuting should make the satellite available again");
}

#[tokio::test]
async fn a_closed_session_leaves_the_announcer() {
    let (p, ann) = connect_and_register(fast()).await;
    drop(p);
    for _ in 0..100 {
        if ann.connected() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the session should deregister when the peer goes away");
}

/// A voice with a configured setting. A line made with `synthesize_voice` uses the voice it was given; a
/// normal line uses the configured one. The voice shows in the length of the audio: A is 1x, B 2x, C 3x.
struct VoiceMouth {
    configured: std::sync::Mutex<String>,
}
impl VoiceMouth {
    fn say(text: &str, voice: &str) -> Vec<i16> {
        let times = match voice {
            "B" => 2,
            "C" => 3,
            _ => 1,
        };
        vec![7; text.len() * 100 * times]
    }
}
impl Synthesizer for VoiceMouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        let voice = self.configured.lock().unwrap().clone();
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(Self::say(text, &voice))
    }
    async fn synthesize_voice(&self, text: &str, voice: &str) -> Result<Vec<i16>, String> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(Self::say(text, voice))
    }
}

async fn session_with(tts: Arc<VoiceMouth>) -> (Peer, Announcer) {
    let s = fast();
    let ann = s.announcer.clone();
    let (near, far) = tokio::io::duplex(1 << 20);
    let (tx, _rx) = mpsc::channel::<Ask>(4);
    tokio::spawn(run_session(far, Arc::new(Ears("x")), tts, tx, s));
    let mut p = Peer {
        io: near,
        dec: Decoder::new(Limits::default()),
    };
    p.hello(None, PROTOCOL_VERSION).await;
    assert!(matches!(p.recv().await, Message::Welcome(_)));
    for _ in 0..100 {
        if ann.connected() == 1 {
            return (p, ann);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the session never registered");
}

/// A preview in voice B, a normal line and a preview in voice C are made at the same moment: each comes out in
/// its own voice and the configured voice is unchanged afterwards.
#[tokio::test]
async fn overlapping_previews_and_a_normal_line_each_use_their_own_voice() {
    let tts = Arc::new(VoiceMouth {
        configured: std::sync::Mutex::new("A".into()),
    });
    let (mut b, ann_b) = session_with(Arc::clone(&tts)).await;
    let (mut n, ann_n) = session_with(Arc::clone(&tts)).await;
    let (mut c, ann_c) = session_with(Arc::clone(&tts)).await;
    let (rb, rn, rc) = tokio::join!(
        ann_b.announce_in_voice("Hello.", Some("B")),
        ann_n.announce_in_voice("Hello.", None),
        ann_c.announce_in_voice("Hello.", Some("C")),
    );
    assert!(rb && rn && rc);
    let stop = |m: &Message| matches!(m, Message::AudioStop);
    let sb = samples(&until(&mut b, stop).await);
    let sn = samples(&until(&mut n, stop).await);
    let sc = samples(&until(&mut c, stop).await);
    assert_eq!(
        (sb, sn, sc),
        (
            "Hello.".len() * 200,
            "Hello.".len() * 100,
            "Hello.".len() * 300
        )
    );
    assert_eq!(&*tts.configured.lock().unwrap(), "A");
}
