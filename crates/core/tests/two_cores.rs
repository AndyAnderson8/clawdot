//! Two cores in one process, each with its own data directory, share nothing (architecture review C6).
//! Before, the pasted keys, the speech registry and source, and the hive were process-wide statics, so a
//! second core would have seen the first one's keys and moved its voice.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::app::{Adapters, Core, Limits, Stores};
use clawdot_core::config::SpeechConfig;
use clawdot_core::sched::SystemClock;
use clawdot_core::secrets::{LLM_KEY, SPEECH_KEY};
use clawdot_core::speech::SpeechOrigin;
use clawdot_core::speechio::build_registered_speech;
use std::path::PathBuf;
use std::sync::Arc;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let d =
            std::env::temp_dir().join(format!("clawdot-two-cores-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &Dir) -> Core {
    Core::open(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
        Stores::in_dir(&dir.0),
    )
    .unwrap()
}

fn speech() -> SpeechConfig {
    SpeechConfig {
        url: Some("http://127.0.0.1:1/v1".into()),
        stt_model: Some("stt".into()),
        tts_model: Some("tts".into()),
        voice: Some("alloy".into()),
        ..SpeechConfig::default()
    }
}

#[test]
fn pasted_keys_belong_to_one_core() {
    let (da, db) = (Dir::new("keys-a"), Dir::new("keys-b"));
    let (a, b) = (open(&da), open(&db));
    a.secrets().set(LLM_KEY, "sk-only-in-core-a-0123").unwrap();
    assert_eq!(
        a.secrets().get(LLM_KEY).as_deref(),
        Some("sk-only-in-core-a-0123")
    );
    assert_eq!(b.secrets().get(LLM_KEY), None, "b must not see a's key");
    b.secrets().set(LLM_KEY, "sk-only-in-core-b-9876").unwrap();
    assert_eq!(
        a.secrets().get(LLM_KEY).as_deref(),
        Some("sk-only-in-core-a-0123"),
        "b's key did not replace a's"
    );
    b.secrets().clear(LLM_KEY).unwrap();
    assert!(
        a.secrets().get(LLM_KEY).is_some(),
        "clearing b left a alone"
    );
    assert!(da.0.join("secrets.json").exists());
}

#[test]
fn each_core_moves_only_its_own_speech_voice() {
    let (da, db) = (Dir::new("voice-a"), Dir::new("voice-b"));
    let (a, b) = (open(&da), open(&db));
    for c in [&a, &b] {
        build_registered_speech(&speech(), c.secrets(), c.env(), c.speech_handles()).unwrap();
    }
    assert_eq!(
        a.speech_handles().swap_voice("nova").as_deref(),
        Some("alloy")
    );
    // b's clients still use the voice they began with, not the one a switched to.
    assert_eq!(
        b.speech_handles().swap_voice("echo").as_deref(),
        Some("alloy")
    );
    assert_eq!(a.speech_handles().swap_voice("x").as_deref(), Some("nova"));
}

#[test]
fn each_core_has_its_own_speech_server_and_key() {
    let (da, db) = (Dir::new("src-a"), Dir::new("src-b"));
    let (a, b) = (open(&da), open(&db));
    let dialect = speech().dialect();
    assert!(a.speech_source().is_none(), "nothing set up yet");
    a.set_speech_origin(SpeechOrigin::new("http://a.example/v1", None, dialect));
    assert!(
        b.speech_source().is_none(),
        "b has no speech server of its own"
    );
    b.set_speech_origin(SpeechOrigin::new("http://b.example/v1", None, dialect));
    a.secrets()
        .set(SPEECH_KEY, "xi-key-for-a-0123456789")
        .unwrap();
    let (sa, sb) = (a.speech_source().unwrap(), b.speech_source().unwrap());
    assert_eq!(sa.url, "http://a.example/v1");
    assert_eq!(sb.url, "http://b.example/v1");
    assert_eq!(sa.key, "xi-key-for-a-0123456789");
    assert_eq!(sb.key, "", "b did not pick up a's pasted key");
}

#[cfg(feature = "hive")]
#[test]
fn each_core_has_its_own_hive() {
    use clawdot_core::hive::{Mode, Options};
    let (da, db) = (Dir::new("hive-a"), Dir::new("hive-b"));
    let (a, b) = (open(&da), open(&db));
    let opts = |port| Options {
        mode: Mode::Off,
        port,
        addr: None,
        publish_name: false,
        name: String::new(),
    };
    assert!(a.hive().is_none());
    let started = clawdot_core::hive::start(&da.0, &opts(3001));
    assert_eq!(started.problem, None);
    a.set_hive(started.hive.unwrap());
    assert!(b.hive().is_none(), "starting a's hive did not start b's");
    let started = clawdot_core::hive::start(&db.0, &opts(3002));
    b.set_hive(started.hive.unwrap());
    let (ja, jb) = (
        a.hive().unwrap().peers_json("A"),
        b.hive().unwrap().peers_json("B"),
    );
    assert_ne!(ja["self"]["id"], jb["self"]["id"], "separate identities");
    assert_eq!(ja["self"]["port"], 3001);
    assert_eq!(jb["self"]["port"], 3002);
}

// Review C10: the key a setting names is looked up in the core's own environment snapshot, so two cores in one
// process can name the same variable and get different keys, and no test has to touch the process environment.
#[test]
fn each_core_resolves_a_named_key_variable_from_its_own_environment() {
    use clawdot_core::env::Env;
    let (da, db) = (Dir::new("env-a"), Dir::new("env-b"));
    let a = open(&da).with_env(Env::from_pairs([("SPEECH_KEY_VAR", "key-for-a")]));
    let b = open(&db).with_env(Env::from_pairs([("SPEECH_KEY_VAR", "key-for-b")]));
    let dialect = speech().dialect();
    for c in [&a, &b] {
        c.set_speech_origin(SpeechOrigin::new(
            "http://x.example/v1",
            Some("SPEECH_KEY_VAR"),
            dialect,
        ));
    }
    assert_eq!(a.speech_source().unwrap().key, "key-for-a");
    assert_eq!(b.speech_source().unwrap().key, "key-for-b");
    assert_eq!(open(&da).speech_source().map(|s| s.key), None);
    assert!(!format!("{:?}", a.env()).contains("key-for"));
}
