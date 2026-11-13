use super::*;
use crate::apiclient::{check_url, multipart, names_in};
use crate::audio::{decode_pcm16, parse_wav, resample_to_16k, wav};
use crate::netpolicy::is_private_host;
use crate::transport::list_models;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn plain_http_is_only_for_the_home_network() {
    for h in [
        "localhost",
        "127.0.0.1",
        "[::1]",
        "192.168.1.5",
        "10.0.0.2",
        "172.16.4.4",
        "dot.local",
        "169.254.1.1",
        "fd00::1",
    ] {
        assert!(is_private_host(h), "{h}");
    }
    for h in [
        "api.openai.com",
        "8.8.8.8",
        "172.32.0.1",
        "example.com",
        "2001:db8::1",
    ] {
        assert!(!is_private_host(h), "{h}");
    }
    assert!(check_url("https://api.openai.com/v1").is_ok());
    assert!(check_url("http://192.168.1.9:9000/v1").is_ok());
    let e = check_url("http://api.openai.com/v1").unwrap_err();
    assert!(e.contains("https://"), "{e}");
}

#[test]
fn wav_files_round_trip_and_wrong_formats_are_explained() {
    let samples = vec![5, -5, 300, -300];
    assert_eq!(parse_wav(&wav(&samples)).unwrap(), samples);
    assert!(parse_wav(b"hello").unwrap_err().contains("not a WAV"));
    let mut stereo = wav(&samples);
    stereo[22] = 2;
    let e = parse_wav(&stereo).unwrap_err();
    assert!(
        e.contains("mono 16-bit PCM at 16000 Hz") && e.contains("2 channel"),
        "{e}"
    );
    let mut truncated = wav(&samples);
    truncated.truncate(30);
    assert!(parse_wav(&truncated).is_err());
    let mut no_data = wav(&samples);
    no_data.truncate(36);
    assert!(parse_wav(&no_data).unwrap_err().contains("no audio"));
}

#[test]
fn hostile_wav_headers_are_errors_or_short_audio_never_a_panic() {
    let samples = vec![1i16, 2, 3, 4];
    let good = wav(&samples);
    // Every prefix of a good file is handled.
    for n in 0..good.len() {
        let _ = parse_wav(&good[..n]);
    }
    // A chunk that declares four gigabytes: the format block is too short, a data block is cut to the file.
    let mut big_fmt = good[..12].to_vec();
    big_fmt.extend(b"fmt ");
    big_fmt.extend(u32::MAX.to_le_bytes());
    assert!(parse_wav(&big_fmt).is_err());
    let mut big_data = good.clone();
    big_data[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(parse_wav(&big_data).unwrap(), samples);
    // A chunk that is skipped may declare any length, and then there is no audio.
    let mut skipped = good[..36].to_vec();
    skipped.extend(b"junk");
    skipped.extend(u32::MAX.to_le_bytes());
    assert!(parse_wav(&skipped).unwrap_err().contains("no audio"));
}

#[test]
fn wav_has_a_valid_header() {
    let w = wav(&[1, -2, 3]);
    assert_eq!(&w[..4], b"RIFF");
    assert_eq!(&w[8..16], b"WAVEfmt ");
    assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
    assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
    assert_eq!(w.len(), 44 + 6);
    assert_eq!(decode_pcm16(&w[44..]), vec![1, -2, 3]);
}

#[test]
fn resampling_changes_length_by_the_ratio_and_keeps_the_shape() {
    let up: Vec<i16> = (0..2400).map(|i| (i % 100) as i16 * 100).collect();
    let down = resample_to_16k(&up, 24_000);
    assert_eq!(down.len(), 1600);
    assert_eq!(down[0], up[0]);
    assert_eq!(resample_to_16k(&up, 16_000), up);
    assert!(resample_to_16k(&[], 24_000).is_empty());
    // 8 kHz input is stretched to twice the length.
    assert_eq!(resample_to_16k(&[0, 100, 200], 8_000).len(), 6);
}

#[test]
fn multipart_has_fields_then_the_file() {
    let b = multipart("XX", &[("model", "m")], b"DATA");
    let s = String::from_utf8(b).unwrap();
    assert!(s.starts_with("--XX\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nm\r\n"));
    assert!(s.contains("name=\"file\"; filename=\"speech.wav\""));
    assert!(s.ends_with("DATA\r\n--XX--\r\n"));
}

/// One canned response; returns the request head and body.
async fn serve_once(
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
) -> (u16, tokio::task::JoinHandle<(String, Vec<u8>)>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let h = tokio::spawn(async move {
        let (mut c, _) = l.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 8192];
        let (head, req_body) = loop {
            let n = c.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&raw[..p]).into_owned();
                let want = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if raw.len() >= p + 4 + want {
                    break (head, raw[p + 4..p + 4 + want].to_vec());
                }
            }
            assert!(n > 0, "client hung up early");
        };
        let mut resp = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        resp.extend_from_slice(&body);
        c.write_all(&resp).await.unwrap();
        (head, req_body)
    });
    (port, h)
}

const T: Duration = Duration::from_secs(5);

#[tokio::test]
async fn a_transcription_uploads_a_wav_and_reads_the_text() {
    let (port, server) = serve_once(
        "200 OK",
        "application/json",
        br#"{"text":" turn on the light "}"#.to_vec(),
    )
    .await;
    let t = HttpTranscriber::new(
        &format!("http://127.0.0.1:{port}/v1"),
        "k3y",
        "whisper-1",
        Some("en"),
        T,
    )
    .unwrap();
    let text = t.transcribe(&[10, 20, 30, 40]).await.unwrap();
    assert_eq!(text, "turn on the light");
    let (head, body) = server.await.unwrap();
    assert!(
        head.starts_with("POST /v1/audio/transcriptions HTTP/1.1"),
        "{head}"
    );
    let lower = head.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer k3y")
            && lower.contains("multipart/form-data; boundary=")
    );
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("name=\"model\"\r\n\r\nwhisper-1")
            && text.contains("name=\"language\"\r\n\r\nen")
    );
    assert!(
        body.windows(4).any(|w| w == b"RIFF"),
        "the audio is sent as a WAV file"
    );
}

#[tokio::test]
async fn transcription_failures_have_clear_messages_without_secrets() {
    let (port, _s) = serve_once(
        "401 Unauthorized",
        "application/json",
        br#"{"error":{"message":"bad key"}}"#.to_vec(),
    )
    .await;
    let t = HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "k3y", "m", None, T).unwrap();
    let e = t.transcribe(&[1]).await.unwrap_err();
    assert_eq!(e, "HTTP 401: bad key");
    assert!(!e.contains("k3y"));
    // A server that echoes the key back in its error does not get it into the message.
    let (port, _s) = serve_once(
        "401 Unauthorized",
        "application/json",
        br#"{"error":{"message":"rejected k3y-echo-1234 sorry"}}"#.to_vec(),
    )
    .await;
    let t = HttpTranscriber::new(
        &format!("http://127.0.0.1:{port}"),
        "k3y-echo-1234",
        "m",
        None,
        T,
    )
    .unwrap();
    let e = t.transcribe(&[1]).await.unwrap_err();
    assert!(
        !e.contains("k3y-echo-1234") && e.contains("rejected"),
        "{e}"
    );
    let (port, _s) = serve_once("200 OK", "text/plain", b"hello".to_vec()).await;
    let t = HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "", "m", None, T).unwrap();
    assert!(t.transcribe(&[1]).await.unwrap_err().contains("not JSON"));
    let (port, _s) = serve_once("200 OK", "application/json", b"{}".to_vec()).await;
    let t = HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "", "m", None, T).unwrap();
    assert!(t.transcribe(&[1]).await.unwrap_err().contains("no `text`"));
}

#[tokio::test]
async fn synthesis_asks_for_pcm_and_returns_16k_audio() {
    let pcm24: Vec<u8> = (0..2400i16).flat_map(|i| (i * 3).to_le_bytes()).collect();
    let (port, server) = serve_once("200 OK", "audio/pcm", pcm24).await;
    let s = HttpSynthesizer::new(
        &format!("http://127.0.0.1:{port}/v1"),
        "",
        "tts-1",
        "alloy",
        24_000,
        T,
    )
    .unwrap();
    let audio = s.synthesize("Hello there.").await.unwrap();
    assert_eq!(audio.len(), 1600);
    let (head, body) = server.await.unwrap();
    assert!(head.starts_with("POST /v1/audio/speech HTTP/1.1"), "{head}");
    let sent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sent["input"], "Hello there.");
    assert_eq!(sent["voice"], "alloy");
    assert_eq!(sent["response_format"], "pcm");
    assert!(
        !head.to_ascii_lowercase().contains("authorization"),
        "no key configured, none sent"
    );
}

// REQ-CFG-080
#[tokio::test]
async fn the_voice_and_language_can_change_while_running() {
    let pcm: Vec<u8> = (0..2400i16).flat_map(|i| i.to_le_bytes()).collect();
    let (port, server) = serve_once("200 OK", "audio/pcm", pcm).await;
    let s = HttpSynthesizer::new(
        &format!("http://127.0.0.1:{port}/v1"),
        "",
        "tts-1",
        "alloy",
        24_000,
        T,
    )
    .unwrap();
    *s.voice_handle().lock().unwrap() = "nova".into();
    s.synthesize("Hi.").await.unwrap();
    let (_, body) = server.await.unwrap();
    let sent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        sent["voice"], "nova",
        "the new voice is used from the next request"
    );

    let (port, server) =
        serve_once("200 OK", "application/json", br#"{"text":"hi"}"#.to_vec()).await;
    let t =
        HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "", "m", Some("en"), T).unwrap();
    *t.language_handle().lock().unwrap() = Some("de".into());
    t.transcribe(&[1, 2, 3]).await.unwrap();
    let (_, body) = server.await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("name=\"language\"\r\n\r\nde"), "{text}");
}

// REQ-CFG-080: a preview overlapping a normal line uses its own voice and leaves the shared one alone.
#[tokio::test]
async fn a_preview_voice_applies_to_one_call_and_never_to_an_overlapping_line() {
    for dialect in [Dialect::OpenAi, Dialect::ElevenLabs, Dialect::Deepgram] {
        let ok = (0..2).map(|_| ("200 OK", pcm_bytes(2400))).collect();
        let (port, server) = serve_seq(ok).await;
        let rate = 24_000;
        let s = HttpSynthesizer::new(
            &format!("http://127.0.0.1:{port}"),
            "k",
            "m",
            "normal-voice",
            rate,
            T,
        )
        .unwrap()
        .with_dialect(dialect)
        .unwrap();
        let (a, b) = tokio::join!(
            s.synthesize_voice("Preview.", "preview-voice"),
            s.synthesize("A reply.")
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(*s.voice_handle().lock().unwrap(), "normal-voice");
        let seen = server.await.unwrap();
        let sent: Vec<String> = seen
            .iter()
            .map(|(head, body)| format!("{head}\n{}", String::from_utf8_lossy(body)))
            .collect();
        let count = |v: &str| sent.iter().filter(|r| r.contains(v)).count();
        assert_eq!(count("preview-voice"), 1, "{dialect:?}: {sent:?}");
        assert_eq!(count("normal-voice"), 1, "{dialect:?}: {sent:?}");
    }
}

// REQ-CFG-081
#[test]
fn lists_of_names_are_read_in_every_common_shape() {
    use serde_json::json;
    assert_eq!(names_in(&json!(["b", "a", "a"])), ["a", "b"]);
    assert_eq!(
        names_in(&json!({"voices": [{"id": "x"}, {"name": "y"}, {"voice_id": "z"}]})),
        ["x", "y", "z"]
    );
    assert_eq!(
        names_in(&json!({"data": [{"id": "gpt-a"}, {"id": "gpt-b"}], "object": "list"})),
        ["gpt-a", "gpt-b"]
    );
    assert_eq!(names_in(&json!({"models": ["m"]})), ["m"]);
    assert!(names_in(&json!({"error": "no"})).is_empty());
    assert!(
        names_in(&json!([1, 2, {"other": "x"}, ""])).is_empty(),
        "only names are taken"
    );
    let long = "x".repeat(129);
    assert!(
        names_in(&json!([long])).is_empty(),
        "a name that is too long is dropped"
    );
}

// REQ-CFG-081
#[tokio::test]
async fn a_server_list_is_fetched_and_a_missing_one_says_so() {
    let (port, server) = serve_once(
        "200 OK",
        "application/json",
        br#"{"data":[{"id":"m2"},{"id":"m1"}]}"#.to_vec(),
    )
    .await;
    let got = list_models(&format!("http://127.0.0.1:{port}/v1"), "")
        .await
        .unwrap();
    assert_eq!(got, ["m1", "m2"]);
    let (head, _) = server.await.unwrap();
    assert!(head.starts_with("GET /v1/models HTTP/1.1"), "{head}");
    let (port, _s) = serve_once("404 Not Found", "application/json", b"{}".to_vec()).await;
    let e = list_voices(&format!("http://127.0.0.1:{port}/v1"), "")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("404"), "{e}");
    let (port, _s) = serve_once("200 OK", "application/json", b"[]".to_vec()).await;
    let e = list_voices(&format!("http://127.0.0.1:{port}/v1"), "")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("did not list any voices"), "{e}");
}

// REQ-VOI-021
#[test]
fn providers_are_named_and_have_their_own_defaults() {
    assert_eq!(Dialect::parse(" ElevenLabs ").unwrap(), Dialect::ElevenLabs);
    assert_eq!(Dialect::parse("deepgram").unwrap(), Dialect::Deepgram);
    assert_eq!(Dialect::parse("openai").unwrap(), Dialect::OpenAi);
    let e = Dialect::parse("acme").unwrap_err();
    assert!(e.contains("openai, elevenlabs, deepgram"), "{e}");
    for d in [Dialect::ElevenLabs, Dialect::Deepgram] {
        assert!(d.default_url().unwrap().starts_with("https://"));
        assert!(d.default_voice().is_some() && d.default_key_env().is_some());
        assert_eq!(Dialect::parse(d.name()).unwrap(), d);
    }
    assert!(
        Dialect::OpenAi.default_url().is_none(),
        "no default server for the OpenAI style"
    );
    // A voice goes into a web address: nothing that could change the path or add a query.
    assert!(plain_id("v", "21m00Tcm4TlvDq8ikWAM").is_ok());
    assert!(plain_id("v", "aura-2-thalia-en").is_ok());
    for bad in ["", "a/b", "a?x=1", "a b", "../x", "a&b"] {
        assert!(plain_id("The voice", bad).is_err(), "{bad}");
    }
}

// REQ-VOI-021
#[test]
fn error_texts_are_plain_and_keep_the_openai_style_as_it_was() {
    assert_eq!(
        Dialect::OpenAi.provider().wire().failure(401, "bad key"),
        "HTTP 401: bad key"
    );
    assert_eq!(
        Dialect::OpenAi.provider().wire().failure(429, ""),
        "HTTP 429"
    );
    let e = Dialect::ElevenLabs
        .provider()
        .wire()
        .failure(401, "Invalid API key");
    assert!(
        e.starts_with("HTTP 401: the key was not accepted") && e.ends_with("(Invalid API key)"),
        "{e}"
    );
    assert!(
        Dialect::Deepgram
            .provider()
            .wire()
            .failure(429, "")
            .contains("wait a little")
    );
    assert_eq!(
        Dialect::Deepgram.provider().wire().failure(500, "boom"),
        "HTTP 500: boom"
    );
    let v = json!({"detail": {"status": "invalid_api_key", "message": "Invalid API key"}});
    assert_eq!(
        Dialect::ElevenLabs.provider().wire().error_message(&v),
        "Invalid API key"
    );
    assert_eq!(
        Dialect::ElevenLabs
            .provider()
            .wire()
            .error_message(&json!({"detail": "plain"})),
        "plain"
    );
    assert_eq!(
        Dialect::ElevenLabs
            .provider()
            .wire()
            .error_message(&json!({"detail": [{"msg": "bad"}]})),
        "bad"
    );
    assert_eq!(
        Dialect::Deepgram
            .provider()
            .wire()
            .error_message(&json!({"err_msg": "Invalid credentials"})),
        "Invalid credentials"
    );
    assert_eq!(
        Dialect::OpenAi
            .provider()
            .wire()
            .error_message(&json!({"message": "x"})),
        ""
    );
}

/// Answer each request in turn with `responses`; returns the request heads and bodies.
async fn serve_seq(
    responses: Vec<(&'static str, Vec<u8>)>,
) -> (u16, tokio::task::JoinHandle<Vec<(String, Vec<u8>)>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let h = tokio::spawn(async move {
        let mut seen = Vec::new();
        for (status, body) in responses {
            let (mut c, _) = l.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 8192];
            let (head, req_body) = loop {
                let n = c.read(&mut buf).await.unwrap();
                raw.extend_from_slice(&buf[..n]);
                if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..p]).into_owned();
                    let want = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if raw.len() >= p + 4 + want {
                        break (head, raw[p + 4..p + 4 + want].to_vec());
                    }
                }
                assert!(n > 0, "client hung up early");
            };
            let mut resp = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
            resp.extend_from_slice(&body);
            c.write_all(&resp).await.unwrap();
            seen.push((head, req_body));
        }
        seen
    });
    (port, h)
}

fn pcm_bytes(n: i16) -> Vec<u8> {
    (0..n).flat_map(|i| (i * 3).to_le_bytes()).collect()
}

fn eleven(port: u16, key: &str, rate: u32) -> HttpSynthesizer {
    HttpSynthesizer::new(
        &format!("http://127.0.0.1:{port}"),
        key,
        "eleven_flash_v2_5",
        "21m00Tcm4TlvDq8ikWAM",
        rate,
        T,
    )
    .unwrap()
    .with_dialect(Dialect::ElevenLabs)
    .unwrap()
}

// REQ-VOI-021
#[tokio::test]
async fn elevenlabs_speaks_with_its_key_header_and_gives_16k_audio() {
    let (port, server) = serve_once("200 OK", "audio/pcm", pcm_bytes(2400)).await;
    let s = eleven(port, "xi-secret", 24_000);
    let audio = s.synthesize("Hello there.").await.unwrap();
    assert_eq!(audio.len(), 1600, "24 kHz becomes 16 kHz");
    let (head, body) = server.await.unwrap();
    assert!(
        head.starts_with(
            "POST /v1/text-to-speech/21m00Tcm4TlvDq8ikWAM?output_format=pcm_24000 HTTP/1.1"
        ),
        "{head}"
    );
    let lower = head.to_ascii_lowercase();
    assert!(lower.contains("xi-api-key: xi-secret"), "{head}");
    assert!(!lower.contains("authorization"), "{head}");
    let sent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sent["text"], "Hello there.");
    assert_eq!(sent["model_id"], "eleven_flash_v2_5");
    // A voice changed while running is the id used next, and an odd id never reaches the address.
    *s.voice_handle().lock().unwrap() = "../admin".into();
    let e = s.synthesize("x").await.unwrap_err();
    assert!(e.contains("not a name this service accepts"), "{e}");
    // 22.05 kHz is asked for as such and converted.
    let (port, server) = serve_once("200 OK", "audio/pcm", pcm_bytes(2205)).await;
    let audio = eleven(port, "k", 22_050).synthesize("x").await.unwrap();
    assert_eq!(audio.len(), 1600);
    assert!(server.await.unwrap().0.contains("output_format=pcm_22050"));
    // A rate ElevenLabs cannot make is refused when set up.
    let bad = HttpSynthesizer::new("http://127.0.0.1:1", "k", "m", "v", 48_000, T)
        .unwrap()
        .with_dialect(Dialect::ElevenLabs);
    assert!(bad.unwrap_err().contains("22050"));
}

// REQ-VOI-021
#[tokio::test]
async fn elevenlabs_failures_are_plain_and_a_missing_key_is_caught_first() {
    let (port, _s) = serve_once(
        "401 Unauthorized",
        "application/json",
        br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#.to_vec(),
    )
    .await;
    let e = eleven(port, "xi-secret", 24_000)
        .synthesize("x")
        .await
        .unwrap_err();
    assert!(
        e.starts_with("HTTP 401: the key was not accepted") && e.contains("Invalid API key"),
        "{e}"
    );
    assert!(!e.contains("xi-secret"));
    let (port, _s) = serve_once(
        "429 Too Many Requests",
        "application/json",
        br#"{"detail":{"status":"too_many_concurrent_requests","message":"Too many concurrent requests"}}"#.to_vec(),
    )
    .await;
    let e = eleven(port, "k", 24_000).synthesize("x").await.unwrap_err();
    assert!(
        e.starts_with("HTTP 429: too many requests") && e.contains("concurrent"),
        "{e}"
    );
    // No key at all: said before anything is sent.
    let e = eleven(1, "", 24_000).synthesize("x").await.unwrap_err();
    assert!(e.contains("No key is set for ElevenLabs"), "{e}");
}

// REQ-VOI-021
#[tokio::test]
async fn elevenlabs_transcribes_with_scribe_and_a_two_letter_language() {
    let (port, server) = serve_once(
        "200 OK",
        "application/json",
        br#"{"language_code":"en","text":" lights on "}"#.to_vec(),
    )
    .await;
    let t = HttpTranscriber::new(
        &format!("http://127.0.0.1:{port}"),
        "xi-k",
        "scribe_v2",
        Some("pt-BR"),
        T,
    )
    .unwrap()
    .with_dialect(Dialect::ElevenLabs);
    assert_eq!(t.transcribe(&[1, 2, 3, 4]).await.unwrap(), "lights on");
    let (head, body) = server.await.unwrap();
    assert!(
        head.starts_with("POST /v1/speech-to-text HTTP/1.1"),
        "{head}"
    );
    assert!(head.to_ascii_lowercase().contains("xi-api-key: xi-k"));
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("name=\"model_id\"\r\n\r\nscribe_v2")
            && text.contains("name=\"language_code\"\r\n\r\npt"),
        "{text}"
    );
    assert!(body.windows(4).any(|w| w == b"RIFF"));
}

// REQ-VOI-021, REQ-VOI-022
#[tokio::test]
async fn elevenlabs_voices_include_cloned_ones_by_name_across_pages() {
    let page1 = br#"{"voices":[{"voice_id":"v2","name":"Zed","category":"premade"},{"voice_id":"v1","name":"My Mum","category":"cloned"}],"has_more":true,"next_page_token":"tok_2"}"#.to_vec();
    let page2 = br#"{"voices":[{"voice_id":"v3","name":"Ana","category":"professional"},{"voice_id":"v1","name":"My Mum","category":"cloned"}],"has_more":false,"next_page_token":null}"#.to_vec();
    let (port, server) = serve_seq(vec![("200 OK", page1), ("200 OK", page2)]).await;
    let (voices, source) = list_voice_details(
        Dialect::ElevenLabs,
        &format!("http://127.0.0.1:{port}"),
        "xi-k",
    )
    .await
    .unwrap();
    assert_eq!(source, "server");
    let got: Vec<(&str, &str, &str)> = voices
        .iter()
        .map(|v| {
            (
                v.id.as_str(),
                v.label.as_str(),
                v.category.as_deref().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("v3", "Ana", "professional"),
            ("v1", "My Mum", "cloned"),
            ("v2", "Zed", "premade")
        ],
        "sorted by name, the name shown while the id is what is sent, no repeats"
    );
    let seen = server.await.unwrap();
    assert!(
        seen[0].0.starts_with("GET /v2/voices?page_size=100"),
        "{}",
        seen[0].0
    );
    assert!(seen[1].0.contains("next_page_token=tok_2"), "{}", seen[1].0);
    assert!(seen[0].0.to_ascii_lowercase().contains("xi-api-key: xi-k"));
    // A bad key is a plain failure; an empty list is said too.
    let (port, _s) = serve_once(
        "401 Unauthorized",
        "application/json",
        br#"{"detail":{"message":"Invalid API key"}}"#.to_vec(),
    )
    .await;
    let e = list_voice_details(
        Dialect::ElevenLabs,
        &format!("http://127.0.0.1:{port}"),
        "bad",
    )
    .await
    .unwrap_err();
    assert!(
        e.is_unauthorized()
            && e.to_string().contains("401")
            && e.to_string().contains("key was not accepted"),
        "{e}"
    );
    let (port, _s) = serve_once(
        "200 OK",
        "application/json",
        br#"{"voices":[],"has_more":false}"#.to_vec(),
    )
    .await;
    let e = list_voice_details(
        Dialect::ElevenLabs,
        &format!("http://127.0.0.1:{port}"),
        "k",
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("did not list any voices"), "{e}");
}

#[test]
fn provider_error_text_is_the_text_shown_and_kinds_are_exclusive() {
    let a = ProviderError::Unauthorized("HTTP 401: x".into());
    let n = ProviderError::NoKey("No key is set for X.".into());
    let o = ProviderError::Other("HTTP 500".into());
    assert_eq!(
        (a.to_string(), n.to_string(), o.to_string()),
        (
            "HTTP 401: x".into(),
            "No key is set for X.".into(),
            "HTTP 500".into()
        )
    );
    assert_eq!(String::from(o.clone()), "HTTP 500");
    assert_eq!(
        [&a, &n, &o].map(|e| (e.is_unauthorized(), e.is_key_problem())),
        [(true, true), (false, true), (false, false)]
    );
}

// The error for a refused key carries the service's words, never the key that was sent.
#[tokio::test]
async fn failures_are_typed_by_kind_and_never_hold_the_key() {
    let key = "sk-secret-0123456789abcdef";
    for (status, want) in [
        ("401 Unauthorized", "unauthorized"),
        ("403 Forbidden", "unauthorized"),
        ("429 Too Many Requests", "other"),
        ("500 Internal Server Error", "other"),
    ] {
        let (port, _s) = serve_once(
            status,
            "application/json",
            br#"{"error":{"message":"status 401 in the text"}}"#.to_vec(),
        )
        .await;
        let e = list_models(&format!("http://127.0.0.1:{port}/v1"), key)
            .await
            .unwrap_err();
        let got = match &e {
            ProviderError::Unauthorized(_) => "unauthorized",
            ProviderError::Other(_) => "other",
            ProviderError::NoKey(_) => "nokey",
        };
        assert_eq!(got, want, "{status}: {e}");
        assert!(!e.to_string().contains(key), "{e}");
    }
    // No key for a service that needs one is its own kind, said before anything is sent.
    let e = list_voice_details(Dialect::ElevenLabs, "http://127.0.0.1:1", "")
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::NoKey(_)), "{e}");
    assert!(e.is_key_problem() && !e.is_unauthorized());
    // A network failure is neither.
    let e = list_models("http://127.0.0.1:1/v1", key).await.unwrap_err();
    assert!(!e.is_key_problem(), "{e}");
    assert!(!e.to_string().contains(key), "{e}");
}

fn deepgram_tts(port: u16, rate: u32) -> HttpSynthesizer {
    HttpSynthesizer::new(
        &format!("http://127.0.0.1:{port}"),
        "dg-key",
        "aura-2",
        "aura-2-thalia-en",
        rate,
        T,
    )
    .unwrap()
    .with_dialect(Dialect::Deepgram)
    .unwrap()
}

// REQ-VOI-021
#[tokio::test]
async fn deepgram_speaks_headerless_pcm_and_gives_16k_audio() {
    let (port, server) = serve_once("200 OK", "audio/l16", pcm_bytes(4800)).await;
    let audio = deepgram_tts(port, 48_000)
        .synthesize("Hello there.")
        .await
        .unwrap();
    assert_eq!(audio.len(), 1600, "48 kHz becomes 16 kHz");
    let (head, body) = server.await.unwrap();
    assert!(
        head.starts_with("POST /v1/speak?model=aura-2-thalia-en&encoding=linear16&container=none&sample_rate=48000 HTTP/1.1"),
        "{head}"
    );
    assert!(
        head.to_ascii_lowercase()
            .contains("authorization: token dg-key"),
        "{head}"
    );
    let sent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sent, json!({"text": "Hello there."}));
    // 16 kHz needs no conversion.
    let (port, _s) = serve_once("200 OK", "audio/l16", pcm_bytes(1600)).await;
    assert_eq!(
        deepgram_tts(port, 16_000)
            .synthesize("x")
            .await
            .unwrap()
            .len(),
        1600
    );
    // 22.05 kHz is not a Deepgram rate.
    let bad = HttpSynthesizer::new("http://127.0.0.1:1", "k", "m", "v", 22_050, T)
        .unwrap()
        .with_dialect(Dialect::Deepgram);
    assert!(bad.unwrap_err().contains("32000"));
}

// REQ-VOI-021
#[tokio::test]
async fn deepgram_transcribes_a_raw_wav_body_and_reads_the_nested_transcript() {
    let ok =
        br#"{"results":{"channels":[{"alternatives":[{"transcript":" turn on the lamp "}]}]}}"#
            .to_vec();
    let (port, server) = serve_once("200 OK", "application/json", ok.clone()).await;
    let t = HttpTranscriber::new(
        &format!("http://127.0.0.1:{port}"),
        "dg-key",
        "nova-3",
        Some("de"),
        T,
    )
    .unwrap()
    .with_dialect(Dialect::Deepgram);
    assert_eq!(t.transcribe(&[5, 6, 7]).await.unwrap(), "turn on the lamp");
    let (head, body) = server.await.unwrap();
    assert!(
        head.starts_with("POST /v1/listen?model=nova-3&smart_format=true&language=de HTTP/1.1"),
        "{head}"
    );
    let lower = head.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: token dg-key") && lower.contains("content-type: audio/wav"),
        "{head}"
    );
    assert_eq!(
        &body[..4],
        b"RIFF",
        "the audio is the body itself, not a form"
    );
    // No language set: ask the service to detect it.
    let (port, server) = serve_once("200 OK", "application/json", ok).await;
    let t = HttpTranscriber::new(
        &format!("http://127.0.0.1:{port}"),
        "dg-key",
        "nova-3",
        None,
        T,
    )
    .unwrap()
    .with_dialect(Dialect::Deepgram);
    t.transcribe(&[1]).await.unwrap();
    assert!(server.await.unwrap().0.contains("detect_language=true"));
    // A bad key and a rate limit.
    let (port, _s) = serve_once(
        "401 Unauthorized",
        "application/json",
        br#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials.","request_id":"r"}"#
            .to_vec(),
    )
    .await;
    let t = HttpTranscriber::new(
        &format!("http://127.0.0.1:{port}"),
        "dg-key",
        "nova-3",
        None,
        T,
    )
    .unwrap()
    .with_dialect(Dialect::Deepgram);
    let e = t.transcribe(&[1]).await.unwrap_err();
    assert!(
        e.starts_with("HTTP 401: the key was not accepted") && e.contains("Invalid credentials"),
        "{e}"
    );
    assert!(!e.contains("dg-key"));
    let (port, _s) = serve_once(
        "429 Too Many Requests",
        "application/json",
        br#"{"err_msg":"slow down"}"#.to_vec(),
    )
    .await;
    let e = deepgram_tts(port, 24_000)
        .synthesize("x")
        .await
        .unwrap_err();
    assert!(
        e.starts_with("HTTP 429: too many requests") && e.contains("slow down"),
        "{e}"
    );
    // The model name cannot change the address.
    let t = HttpTranscriber::new("http://127.0.0.1:1", "k", "nova&x=1", None, T)
        .unwrap()
        .with_dialect(Dialect::Deepgram);
    assert!(t.transcribe(&[1]).await.unwrap_err().contains("not a name"));
}

// REQ-VOI-022
#[tokio::test]
async fn deepgram_has_a_built_in_voice_list() {
    let (voices, source) = list_voice_details(Dialect::Deepgram, "https://api.deepgram.com", "")
        .await
        .unwrap();
    assert_eq!(source, "builtin");
    assert!(
        voices
            .iter()
            .any(|v| v.id == "aura-2-thalia-en" && v.label == "Thalia")
    );
    assert!(voices.iter().all(|v| plain_id("v", &v.id).is_ok()));
}

// REQ-VOI-022
#[tokio::test]
async fn the_openai_style_voice_list_keeps_names_as_labels() {
    let (port, _s) = serve_once("200 OK", "application/json", br#"["b","a"]"#.to_vec()).await;
    let (voices, source) =
        list_voice_details(Dialect::OpenAi, &format!("http://127.0.0.1:{port}"), "")
            .await
            .unwrap();
    assert_eq!(source, "server");
    assert_eq!(
        voices
            .iter()
            .map(|v| (v.id.as_str(), v.label.as_str()))
            .collect::<Vec<_>>(),
        [("a", "a"), ("b", "b")]
    );
}

#[tokio::test]
async fn synthesis_rejects_empty_audio_and_bad_settings() {
    let (port, _s) = serve_once("200 OK", "audio/pcm", Vec::new()).await;
    let s =
        HttpSynthesizer::new(&format!("http://127.0.0.1:{port}"), "", "m", "v", 24_000, T).unwrap();
    assert!(s.synthesize("x").await.unwrap_err().contains("no audio"));
    assert!(HttpSynthesizer::new("http://127.0.0.1:1", "", "m", "v", 100, T).is_err());
    assert!(HttpTranscriber::new("http://speech.example.com/v1", "", "m", None, T).is_err());
}

#[tokio::test]
async fn a_silent_server_times_out_instead_of_hanging() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let _hold = tokio::spawn(async move {
        let (_c, _) = l.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let s = HttpSynthesizer::new(
        &format!("http://127.0.0.1:{port}"),
        "",
        "m",
        "v",
        24_000,
        Duration::from_millis(300),
    )
    .unwrap();
    assert!(s.synthesize("x").await.unwrap_err().contains("no response"));
}

#[tokio::test]
async fn the_client_reads_the_stored_secret_it_is_told_to_and_no_other() {
    let dir = std::env::temp_dir().join(format!("clawdot-client-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let secrets = crate::secrets::Secrets::in_dir(&dir);
    secrets.set("other_key", "stored-other").unwrap();
    secrets
        .set(crate::secrets::SPEECH_KEY, "stored-speech")
        .unwrap();
    for (name, want) in [
        ("other_key", "Bearer stored-other"),
        (crate::secrets::SPEECH_KEY, "Bearer stored-speech"),
        ("never_set", "Bearer from-config"),
    ] {
        let (port, server) = serve_once("200 OK", "application/json", b"{}".to_vec()).await;
        let mut c = crate::apiclient::Client::new(
            &format!("http://127.0.0.1:{port}"),
            "from-config",
            T,
            &crate::apiclient::Bearer,
        )
        .unwrap();
        c.set_stored_key(secrets.clone(), name);
        c.send(crate::apiclient::Call::get("/x")).await.unwrap();
        let (head, _) = server.await.unwrap();
        assert!(
            head.to_lowercase().contains(&want.to_lowercase()),
            "{name}: {head}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_model_list_words_its_own_refusals_and_the_speech_clients_keep_theirs() {
    // Plain http across the internet: the model list names the key only, speech names the key and the audio.
    let e = list_models("http://api.openai.com/v1", "k")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("the key must not cross"), "{e}");
    assert!(!e.to_string().contains("audio"), "{e}");
    let e = HttpTranscriber::new("http://speech.example.com/v1", "", "m", None, T).unwrap_err();
    assert!(e.contains("the key and audio must not cross"), "{e}");
    // The model list's wire names the language model server and the model settings, not the speech ones.
    let wire = crate::apiclient::ModelList;
    assert_eq!(wire.title(), "the language model server");
    assert!(wire.key_hint().contains("model settings"));
    assert_eq!(crate::apiclient::Bearer.title(), "the speech server");
    assert!(
        crate::apiclient::Bearer
            .key_hint()
            .contains("voice settings")
    );
}

#[tokio::test]
async fn the_no_key_message_uses_the_wire_s_title_and_hint() {
    struct KeyedModels;
    impl Wire for KeyedModels {
        fn title(&self) -> &'static str {
            crate::apiclient::ModelList.title()
        }
        fn key_hint(&self) -> &'static str {
            crate::apiclient::ModelList.key_hint()
        }
        fn needs_key(&self) -> bool {
            true
        }
        fn auth_header(&self, key: &str) -> (&'static str, String) {
            crate::apiclient::ModelList.auth_header(key)
        }
    }
    let c = crate::apiclient::Client::new("http://127.0.0.1:1", "", T, &KeyedModels).unwrap();
    let e = c.send(crate::apiclient::Call::get("/x")).await.unwrap_err();
    assert!(matches!(e, ProviderError::NoKey(_)), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("the language model server") && text.contains("model settings"),
        "{text}"
    );
    assert!(
        !text.contains("speech") && !text.contains("voice"),
        "{text}"
    );
}

#[tokio::test]
async fn a_response_over_four_mebibytes_is_refused_and_one_at_the_cap_is_kept() {
    const CAP: usize = 4 * 1024 * 1024;
    let (port, _s) = serve_once("200 OK", "audio/pcm", vec![0u8; CAP + 2]).await;
    let s =
        HttpSynthesizer::new(&format!("http://127.0.0.1:{port}"), "", "m", "v", 24_000, T).unwrap();
    let e = s.synthesize("x").await.unwrap_err();
    assert!(e.contains("response too large"), "{e}");
    let (port, _s) = serve_once("200 OK", "audio/pcm", vec![0u8; CAP]).await;
    let s =
        HttpSynthesizer::new(&format!("http://127.0.0.1:{port}"), "", "m", "v", 24_000, T).unwrap();
    assert!(s.synthesize("x").await.is_ok());
}
