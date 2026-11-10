//! The published JSON Schemas (`schemas/satellite/`) and the Rust types must agree (REQ-SAT-012).
//!
//! No schema library: the schemas use a small subset of JSON Schema (type, const, enum, required, properties,
//! items, `$ref`, and numeric and length bounds), which `validate` below implements in about sixty lines. `pattern`
//! is deliberately not evaluated; the one pattern (stream URLs) is covered by the parser tests in `message.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_protocol::message::{
    AudioCodec, AudioFormat, Button, Capabilities, ErrorReport, Hello, Indicator, MediaAction,
    Message, PROTOCOL_VERSION, Reject, Wake, Welcome,
};
use clawdot_protocol::{Decoder, Limits, encode};
use serde_json::{Value, json};

const SCHEMA: &str = include_str!("../../../schemas/satellite/frame-0.1.schema.json");

fn resolve<'a>(root: &'a Value, reference: &str) -> &'a Value {
    let path = reference.strip_prefix("#/").expect("local $ref");
    path.split('/').fold(root, |v, key| {
        v.get(key)
            .unwrap_or_else(|| panic!("dangling $ref {reference}"))
    })
}

/// Validate `value` against `schema`; the error names the first failing keyword.
fn validate(root: &Value, schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        return validate(root, resolve(root, r), value);
    }
    if let Some(c) = schema.get("const") {
        if c != value {
            return Err(format!("expected {c}, got {value}"));
        }
    }
    if let Some(Value::Array(options)) = schema.get("enum") {
        if !options.contains(value) {
            return Err(format!("{value} is not one of {options:?}"));
        }
    }
    if let Some(t) = schema.get("type").and_then(Value::as_str) {
        let ok = match t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            other => panic!("schema uses unsupported type {other}"),
        };
        if !ok {
            return Err(format!("expected {t}, got {value}"));
        }
    }
    if let Some(n) = value.as_i64() {
        if schema
            .get("minimum")
            .and_then(Value::as_i64)
            .is_some_and(|m| n < m)
            || schema
                .get("maximum")
                .and_then(Value::as_i64)
                .is_some_and(|m| n > m)
        {
            return Err(format!("{n} is out of range"));
        }
    }
    if let Some(s) = value.as_str() {
        let len = s.chars().count();
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|m| (len as u64) < m)
            || schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|m| (len as u64) > m)
        {
            return Err(format!("string of {len} characters is out of range"));
        }
    }
    if let (Some(obj), Some(Value::Array(required))) = (value.as_object(), schema.get("required")) {
        for key in required.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                return Err(format!("missing `{key}`"));
            }
        }
    }
    if let (Some(obj), Some(Value::Object(props))) = (value.as_object(), schema.get("properties")) {
        for (key, sub) in props {
            if let Some(v) = obj.get(key) {
                validate(root, sub, v).map_err(|e| format!("{key}: {e}"))?;
            }
        }
    }
    if let (Some(items), Some(sub)) = (value.as_array(), schema.get("items")) {
        for v in items {
            validate(root, sub, v)?;
        }
    }
    Ok(())
}

fn schema() -> Value {
    serde_json::from_str(SCHEMA).unwrap()
}

/// Validate a header against the definition named by its own `type`.
fn check(header: &Value) -> Result<(), String> {
    let root = schema();
    let kind = header["type"].as_str().ok_or("no type")?;
    let def = root["$defs"]
        .get(kind)
        .ok_or_else(|| format!("no schema for `{kind}`"))?;
    validate(&root, &root, header)?;
    validate(&root, def, header)
}

fn header_of(msg: &Message) -> Value {
    let bytes = encode(&msg.to_frame()).unwrap();
    let line = bytes.split(|&b| b == b'\n').next().unwrap();
    serde_json::from_slice(line).unwrap()
}

fn every_message() -> Vec<Message> {
    vec![
        Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: "dot-1".into(),
            name: Some("Kitchen".into()),
            token: Some("secret".into()),
            capabilities: Capabilities {
                wake_word: true,
                media_url: true,
                audio_out: vec![AudioCodec::Pcm, AudioCodec::Opus],
                ..Capabilities::default()
            },
        }),
        Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: "bare".into(),
            name: None,
            token: None,
            capabilities: Capabilities::default(),
        }),
        Message::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            session_id: "s1".into(),
            audio_out: AudioCodec::Pcm,
        }),
        Message::Reject(Reject {
            reason: "unauthorized".into(),
        }),
        Message::Ping,
        Message::Pong,
        Message::AudioStart(AudioFormat::PCM_16K_MONO),
        Message::AudioChunk {
            format: AudioFormat::PCM_16K_MONO,
            timestamp_ms: Some(40),
            payload: vec![0; 640],
        },
        Message::AudioChunk {
            format: AudioFormat::PCM_16K_MONO,
            timestamp_ms: None,
            payload: vec![0; 640],
        },
        Message::AudioStop,
        Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: Some(5),
        }),
        Message::Wake(Wake {
            name: "button".into(),
            timestamp_ms: None,
        }),
        Message::Button(Button {
            name: "action".into(),
            long: false,
        }),
        Message::MuteChanged { muted: true },
        Message::PlaybackDone { id: None },
        Message::PlaybackDone {
            id: Some("tts-1".into()),
        },
        Message::Error(ErrorReport {
            text: "mic".into(),
            code: Some("mic".into()),
        }),
        Message::StreamStart,
        Message::StreamStop,
        Message::StopPlayback,
        Message::SetVolume { level: 0 },
        Message::SetVolume { level: 100 },
        Message::SetIndicator(Indicator::Idle),
        Message::SetIndicator(Indicator::WakeDetected),
        Message::SetIndicator(Indicator::Listening),
        Message::SetIndicator(Indicator::Thinking),
        Message::SetIndicator(Indicator::Speaking),
        Message::SetIndicator(Indicator::Error),
        Message::SetIndicator(Indicator::Muted),
        Message::SetIndicator(Indicator::Notification),
        Message::SetMute { muted: false },
        Message::PlayUrl {
            url: "https://r.example/jazz".into(),
            title: "Jazz".into(),
        },
        Message::MediaControl(MediaAction::Pause),
        Message::MediaControl(MediaAction::Resume),
        Message::AdminReset,
        Message::AdminResetWindow {
            open_secs: 300,
            reason: None,
        },
        Message::AdminResetWindow {
            open_secs: 0,
            reason: Some("Wait a minute and try again.".into()),
        },
    ]
}

#[test]
fn req_sat_012_every_message_the_code_can_send_validates_against_the_schema() {
    for msg in every_message() {
        let header = header_of(&msg);
        check(&header).unwrap_or_else(|e| panic!("{header}: {e}"));
    }
}

#[test]
fn req_sat_012_every_known_event_has_a_schema_and_every_schema_is_exercised() {
    let root = schema();
    let defs = root["$defs"].as_object().unwrap();
    let exercised: Vec<String> = every_message()
        .iter()
        .map(|m| header_of(m)["type"].as_str().unwrap().to_owned())
        .collect();
    let events: Vec<&String> = defs
        .keys()
        .filter(|k| defs[*k]["properties"]["type"]["const"].is_string())
        .collect();
    for kind in &events {
        assert!(
            exercised.contains(kind),
            "no test message produces `{kind}`"
        );
    }
    for kind in &exercised {
        assert!(
            events.contains(&kind),
            "`{kind}` is on the wire but has no schema"
        );
    }
}

#[test]
fn req_sat_012_the_parser_and_the_schema_reject_the_same_malformed_messages() {
    let bad = [
        json!({"type": "clawdot.set-volume", "data": {"level": 101}}),
        json!({"type": "clawdot.set-volume", "data": {"level": -1}}),
        json!({"type": "clawdot.set-volume", "data": {"level": "loud"}}),
        json!({"type": "clawdot.set-volume"}),
        json!({"type": "clawdot.set-indicator", "data": {"state": "dancing"}}),
        json!({"type": "clawdot.set-mute", "data": {"muted": "yes"}}),
        json!({"type": "clawdot.mute-changed", "data": {}}),
        json!({"type": "clawdot.media-control", "data": {"action": "explode"}}),
        json!({"type": "clawdot.play-url", "data": {"url": "http://x/", "title": "t".repeat(201)}}),
        json!({"type": "clawdot.play-url", "data": {"url": format!("http://x/{}", "a".repeat(2048)), "title": "t"}}),
        json!({"type": "clawdot.play-url", "data": {"title": "no url"}}),
        json!({"type": "clawdot.hello"}),
        json!({"type": "clawdot.hello", "data": {"device_id": "d"}}),
        json!({"type": "clawdot.hello", "data": {"protocol": {"major": 0, "minor": 1}}}),
        json!({"type": "clawdot.hello", "data": {"protocol": {"major": 0, "minor": 1}, "device_id": "d", "capabilities": {"leds": "yes"}}}),
        json!({"type": "clawdot.welcome", "data": {"protocol": {"major": 0, "minor": 1}, "session_id": "s", "audio_out": "mp3"}}),
        json!({"type": "clawdot.reject", "data": {}}),
        json!({"type": "detection", "data": {}}),
        json!({"type": "clawdot.button", "data": {"long": true}}),
        json!({"type": "error", "data": {"code": "x"}}),
        json!({"type": "audio-start", "data": {"rate": 16000}}),
    ];
    for header in &bad {
        assert!(check(header).is_err(), "the schema accepted {header}");
        let frame = {
            let mut f = clawdot_protocol::Frame::new(header["type"].as_str().unwrap());
            if let Some(Value::Object(d)) = header.get("data") {
                f = f.with_data(d.clone());
            }
            f
        };
        assert!(
            Message::from_frame(frame).is_err(),
            "the parser accepted {header}"
        );
    }
}

#[test]
fn req_sat_009_unknown_events_have_no_schema_and_are_ignored_by_the_parser() {
    let header = json!({"type": "clawdot.from-the-future", "data": {"x": 1}});
    assert!(
        check(&header).is_err(),
        "there is deliberately no schema for it"
    );
    let mut d = Decoder::new(Limits::default());
    d.push(&encode(&clawdot_protocol::Frame::new("clawdot.from-the-future")).unwrap());
    let frame = d.next_frame().unwrap().unwrap();
    assert!(matches!(
        Message::from_frame(frame).unwrap(),
        Message::Unknown(_)
    ));
}

#[test]
fn req_sat_009_extra_fields_do_not_fail_the_schema() {
    let header =
        json!({"type": "detection", "data": {"name": "hey", "future": [1, 2]}, "extra": true});
    check(&header).unwrap();
}

#[test]
fn req_sat_002_a_hello_without_capabilities_is_valid() {
    // REQ-SAT-013: the mandatory set needs no capability declarations at all.
    check(&json!({"type": "clawdot.hello", "data": {"protocol": {"major": 0, "minor": 1}, "device_id": "bare"}})).unwrap();
}
