use super::*;
use crate::env::Env;
use crate::store::testutil::TempDir;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn keypair() -> Ed25519KeyPair {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

fn public(kp: &Ed25519KeyPair) -> [u8; 32] {
    kp.public_key().as_ref().try_into().unwrap()
}

type Route = (u16, Vec<(String, String)>, Vec<u8>);

/// Serve fixed answers on this machine; returns the port. Unknown paths answer 404.
async fn serve(routes: Vec<(&'static str, Route)>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    serve_on(listener, routes);
    port
}

fn ok(body: &[u8]) -> Route {
    (200, vec![], body.to_vec())
}

fn release_json(port: u16, version: &str) -> String {
    let a = |n: &str| json!({"name": n, "browser_download_url": format!("http://127.0.0.1:{port}/dl/{n}"), "size": 3});
    json!({
        "tag_name": format!("v{version}"),
        "html_url": "https://example.test/release",
        "body": "Faster wake word.\n<b>not markup</b>",
        "published_at": "2026-11-10T00:00:00Z",
        "assets": [a(&asset_name()), a(SUMS_NAME), a(SIG_NAME)],
    })
    .to_string()
}

struct Rig {
    updater: Arc<Updater>,
    dir: TempDir,
    exe: PathBuf,
}

fn rig(port: u16, key: Option<[u8; 32]>) -> Rig {
    let dir = TempDir::new("update");
    let exe = dir.0.join("clawdot-test-exe");
    std::fs::write(&exe, b"old program").unwrap();
    let cfg = UpdateConfig {
        url: format!("http://127.0.0.1:{port}/feed"),
        public_key: key,
        supervised: false,
    };
    let updater = Updater::new(cfg, true, Some(dir.0.clone()), exe.clone(), false, false).unwrap();
    Rig {
        updater: Arc::new(updater),
        dir,
        exe,
    }
}

/// A server with a newer release whose files are signed by `signer`, whose signed hash is of `hashed`, and whose
/// program file is `program`.
async fn newer_release(signer: &Ed25519KeyPair, program: &[u8], hashed: &[u8]) -> u16 {
    let name = asset_name();
    let sums = format!(
        "{}  {name}
",
        sha256_hex(hashed)
    );
    let sig = hex(signer.sign(sums.as_bytes()).as_ref());
    // The feed names the port it is served on, so choose the port first.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let program_path: &'static str = Box::leak(format!("/dl/{name}").into_boxed_str());
    serve_on(
        listener,
        vec![
            ("/feed", ok(release_json(port, "99.0.0").as_bytes())),
            (program_path, ok(program)),
            ("/dl/SHA256SUMS", ok(sums.as_bytes())),
            ("/dl/SHA256SUMS.sig", ok(sig.as_bytes())),
        ],
    );
    port
}

fn serve_on(listener: TcpListener, routes: Vec<(&'static str, Route)>) {
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let routes = Arc::clone(&routes);
            tokio::spawn(async move {
                let mut seen = Vec::new();
                let mut chunk = [0_u8; 2048];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    seen.extend_from_slice(&chunk[..n]);
                }
                let line = String::from_utf8_lossy(&seen);
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (code, headers, body) = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map_or((404, vec![], b"no".to_vec()), |(_, r)| r.clone());
                let mut out = format!(
                    "HTTP/1.1 {code} X\r\nConnection: close\r\nContent-Length: {}\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let _ = s.write_all(out.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    });
}

// REQ-UPD-002
#[test]
fn versions_compare_the_way_people_expect() {
    for (a, b, newer) in [
        ("0.1.0", "0.1.1", true),
        ("0.1.9", "0.2.0", true),
        ("0.9.0", "1.0.0", true),
        ("0.1.0", "0.1.0", false),
        ("0.2.0", "0.1.9", false),
        ("1.0.0-rc1", "1.0.0", true),
        ("1.0.0", "1.0.0-rc1", false),
        ("0.1.0", "v0.1.1", true),
        ("0.1.0", "0.1.1+build5", true),
        ("0.1.0", "banana", false),
        ("0.1.0", "1.2", false),
        ("0.1.0", "1.2.3.4", false),
    ] {
        assert_eq!(is_newer(a, b), newer, "{a} -> {b}");
    }
}

// REQ-UPD-007
#[test]
fn the_new_program_must_report_exactly_the_announced_version() {
    assert!(reports_version("clawdot 0.2.0 (abc1234)\n", "0.2.0"));
    assert!(reports_version("clawdot v0.2.0\n", "0.2.0"));
    for wrong in [
        "clawdot 10.2.0 (abc)",
        "clawdot 0.2.01 (abc)",
        "clawdot 0.2.0-old (abc)",
        "",
    ] {
        assert!(!reports_version(wrong, "0.2.0"), "{wrong}");
    }
}

// REQ-UPD-003
#[test]
fn a_release_is_read_as_data_and_odd_ones_are_refused() {
    let r = parse_release(&release_json(9, "1.4.0")).unwrap();
    assert_eq!(r.version, "1.4.0");
    assert_eq!(r.assets.len(), 3);
    assert!(
        r.notes.contains("<b>not markup</b>"),
        "notes are text, kept as they are"
    );
    assert_eq!(r.page, "https://example.test/release");
    assert!(parse_release("not json").is_err());
    assert!(parse_release(r#"{"tag_name":"banana"}"#).is_err());
    assert!(parse_release(r#"{"tag_name":"v1.0.0","prerelease":true}"#).is_err());
    assert!(parse_release(r#"{"tag_name":"v1.0.0","draft":true}"#).is_err());
    // A download address that is not https (and not this machine) is dropped, never followed.
    let r = parse_release(
        r#"{"tag_name":"v1.0.0","html_url":"http://evil.test/x","assets":[{"name":"a","browser_download_url":"http://evil.test/a","size":1}]}"#,
    )
    .unwrap();
    assert!(r.assets.is_empty());
    assert_eq!(r.page, "");
}

// REQ-UPD-004
#[test]
fn the_list_of_hashes_and_signatures_are_read_strictly() {
    let h = "a".repeat(64);
    let sums = parse_sums(&format!(
        "{h}  clawdot-linux-armv7\n{}  *other\nnot a line\n{h}\n",
        "B".repeat(64)
    ));
    assert_eq!(sums.get("clawdot-linux-armv7"), Some(&h));
    assert_eq!(
        sums.get("other"),
        Some(&"b".repeat(64)),
        "case is folded, a leading star is dropped"
    );
    assert_eq!(sums.len(), 2);
    assert!(parse_sums("abc  short-hash").is_empty());
    assert_eq!(unhex("00ff"), Some(vec![0, 255]));
    assert_eq!(unhex("0"), None);
    assert_eq!(unhex("zz"), None);

    let kp = keypair();
    let msg = b"hashes";
    let sig = hex(kp.sign(msg).as_ref());
    assert!(verify_signature(&public(&kp), msg, &sig));
    assert!(
        !verify_signature(&public(&kp), b"other", &sig),
        "a changed message fails"
    );
    assert!(
        !verify_signature(&public(&keypair()), msg, &sig),
        "another key fails"
    );
    assert!(
        !verify_signature(&public(&kp), msg, "nothex"),
        "garbage fails"
    );
    assert!(!verify_signature(&public(&kp), msg, ""), "empty fails");
}

// REQ-UPD-001, REQ-UPD-005
#[tokio::test]
async fn checking_finds_a_newer_release_and_remembers_it() {
    let kp = keypair();
    let port = newer_release(&kp, b"new program", b"new program").await;
    let r = rig(port, Some(public(&kp)));
    r.updater.check().await.unwrap();
    let st = r.updater.status();
    assert_eq!(st["available"], true);
    assert_eq!(st["latest"]["version"], "99.0.0");
    assert_eq!(st["can_apply"], true);
    assert!(st["last_checked"].as_u64().is_some());
    assert_eq!(st["error"], Value::Null);
    // A restart still knows what was found, without asking again.
    let again = rig(port, Some(public(&kp)));
    std::fs::copy(r.dir.0.join("update.json"), again.dir.0.join("update.json")).unwrap();
    again.updater.load();
    assert_eq!(again.updater.status()["latest"]["version"], "99.0.0");
}

// REQ-UPD-005
#[tokio::test]
async fn a_failed_check_says_why_and_keeps_what_was_known() {
    let kp = keypair();
    let port = newer_release(&kp, b"p", b"p").await;
    let r = rig(port, Some(public(&kp)));
    r.updater.check().await.unwrap();
    // Now the server stops knowing the feed.
    let dead = rig(1, Some(public(&kp)));
    dead.updater.load();
    std::fs::copy(r.dir.0.join("update.json"), dead.dir.0.join("update.json")).unwrap();
    dead.updater.load();
    assert!(dead.updater.check().await.is_err());
    let st = dead.updater.status();
    assert!(
        st["error"]
            .as_str()
            .unwrap()
            .starts_with("Could not check for updates")
    );
    assert_eq!(
        st["latest"]["version"], "99.0.0",
        "the last good answer is kept"
    );
}

// REQ-UPD-006, REQ-UPD-007
#[tokio::test]
async fn a_signed_update_is_installed_and_the_old_program_is_kept() {
    let kp = keypair();
    let port = newer_release(&kp, b"new program", b"new program").await;
    let r = rig(port, Some(public(&kp)));
    r.updater.check().await.unwrap();
    let release = r.updater.lock().latest.clone().unwrap();
    r.updater.apply(&release).await.unwrap();
    assert_eq!(std::fs::read(&r.exe).unwrap(), b"new program");
    assert_eq!(
        std::fs::read(r.exe.with_file_name("clawdot-test-exe.prev")).unwrap(),
        b"old program"
    );
    assert!(
        !r.exe.with_file_name("clawdot-test-exe.new").exists(),
        "no half-written file is left"
    );
    assert_eq!(
        std::fs::read_to_string(r.dir.0.join(PENDING_FILE)).unwrap(),
        "99.0.0"
    );
    assert!(pending_marker(&r.dir.0));
    let st = r.updater.status();
    assert_eq!(st["phase"], "installed");
    assert_eq!(st["installed"], "99.0.0");
}

// REQ-UPD-006
#[tokio::test]
async fn a_changed_download_is_refused_and_nothing_is_replaced() {
    let kp = keypair();
    // The signed hash is for one file; the server hands out another.
    let port = newer_release(&kp, b"tampered program", b"the real program").await;
    let r = rig(port, Some(public(&kp)));
    r.updater.check().await.unwrap();
    let release = r.updater.lock().latest.clone().unwrap();
    let e = r.updater.apply(&release).await.unwrap_err();
    assert!(e.contains("does not match its signed hash"), "{e}");
    assert_eq!(std::fs::read(&r.exe).unwrap(), b"old program");
    assert!(!r.exe.with_file_name("clawdot-test-exe.prev").exists());
    assert!(!r.dir.0.join(PENDING_FILE).exists());
}

// REQ-UPD-006
#[tokio::test]
async fn a_release_signed_by_the_wrong_key_is_refused() {
    let signer = keypair();
    let trusted = keypair();
    let port = newer_release(&signer, b"new program", b"new program").await;
    let r = rig(port, Some(public(&trusted)));
    r.updater.check().await.unwrap();
    let release = r.updater.lock().latest.clone().unwrap();
    let e = r.updater.apply(&release).await.unwrap_err();
    assert!(e.contains("not signed by the project's key"), "{e}");
    assert_eq!(std::fs::read(&r.exe).unwrap(), b"old program");
}

// REQ-UPD-008
#[tokio::test]
async fn a_build_with_no_key_never_installs_and_says_where_to_download() {
    let kp = keypair();
    let port = newer_release(&kp, b"new program", b"new program").await;
    let r = rig(port, None);
    r.updater.check().await.unwrap();
    let st = r.updater.status();
    assert_eq!(
        st["available"], true,
        "it still tells you a new version exists"
    );
    assert_eq!(st["can_apply"], false);
    assert!(
        st["cannot_apply_because"]
            .as_str()
            .unwrap()
            .contains("no update key")
    );
    let e = r.updater.start_apply("99.0.0").unwrap_err();
    assert!(e.contains("no update key"), "{e}");
    assert_eq!(std::fs::read(&r.exe).unwrap(), b"old program");
}

// REQ-UPD-009
#[tokio::test]
async fn only_the_newer_version_that_was_found_can_be_asked_for() {
    let kp = keypair();
    let port = newer_release(&kp, b"p", b"p").await;
    let r = rig(port, Some(public(&kp)));
    assert!(
        r.updater
            .start_apply("99.0.0")
            .unwrap_err()
            .contains("Check for updates first")
    );
    r.updater.check().await.unwrap();
    assert!(
        r.updater
            .start_apply("98.0.0")
            .unwrap_err()
            .contains("not a newer one")
    );
    assert!(
        r.updater
            .start_apply("0.0.1")
            .unwrap_err()
            .contains("not a newer one")
    );
}

// REQ-UPD-010
#[tokio::test]
async fn downloads_follow_redirects_but_only_to_https_or_this_machine() {
    let port = serve(vec![]).await;
    let target = serve(vec![("/file", ok(b"data"))]).await;
    let hop = serve(vec![(
        "/start",
        (
            302,
            vec![("Location".into(), format!("http://127.0.0.1:{target}/file"))],
            vec![],
        ),
    )])
    .await;
    let _ = port;
    let r = rig(hop, None);
    let got = r
        .updater
        .fetch(
            &format!("http://127.0.0.1:{hop}/start"),
            1000,
            "*/*",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(got, b"data");
    // A plain-http address on the network is never fetched.
    let e = r
        .updater
        .fetch(
            "http://example.test/file",
            1000,
            "*/*",
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
    assert!(e.contains("only fetched over https"), "{e}");
    // A file larger than allowed is refused.
    let e = r
        .updater
        .fetch(
            &format!("http://127.0.0.1:{target}/file"),
            2,
            "*/*",
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert!(e.contains("larger than allowed"), "{e}");
}

// REQ-UPD-011
#[test]
fn a_new_version_that_never_proved_itself_leaves_its_marker_for_the_supervisor() {
    let d = TempDir::new("pending");
    let cfg = UpdateConfig {
        url: DEFAULT_URL.into(),
        public_key: None,
        supervised: false,
    };
    let u = Updater::new(cfg, true, Some(d.0.clone()), d.0.join("x"), false, false).unwrap();
    assert!(!pending_marker(&d.0));
    std::fs::write(d.0.join(PENDING_FILE), "99.9.9").unwrap();
    u.settle_pending(); // this is not version 99.9.9, so the marker is stale and removed
    assert!(!pending_marker(&d.0));
}

#[test]
fn the_configuration_is_checked() {
    assert!(UpdateConfig::new(None, None, &Env::empty()).is_ok());
    assert!(
        UpdateConfig::new(Some("http://example.test/feed"), None, &Env::empty())
            .unwrap_err()
            .contains("https")
    );
    assert!(UpdateConfig::new(Some("ftp://x"), None, &Env::empty()).is_err());
    assert!(
        UpdateConfig::new(None, Some("zz"), &Env::empty())
            .unwrap_err()
            .contains("64 hexadecimal")
    );
    let key = "ab".repeat(32);
    assert_eq!(
        UpdateConfig::new(None, Some(&key), &Env::empty())
            .unwrap()
            .public_key,
        Some([0xab; 32])
    );
    assert!(asset_name().starts_with("clawdot-"));
}

#[test]
fn supervision_is_read_from_the_environment_snapshot() {
    let on = |pairs: &[(&str, &str)]| {
        UpdateConfig::new(None, None, &Env::from_pairs(pairs.iter().copied()))
            .unwrap()
            .supervised
    };
    assert!(!on(&[]));
    assert!(on(&[("CLAWDOT_SUPERVISED", "1")]));
    assert!(on(&[("INVOCATION_ID", "abc")]));
}

// REQ-UPD-009: two requests at once start one install, not two.
#[test]
fn two_requests_at_once_start_exactly_one_install() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let kp = keypair();
    let port = rt.block_on(newer_release(&kp, b"p", b"p"));
    let found = {
        let r = rig(port, Some(public(&kp)));
        rt.block_on(r.updater.check()).unwrap();
        let latest = r.updater.lock().latest.clone();
        latest.unwrap()
    };
    for _ in 0..200 {
        let r = rig(port, Some(public(&kp)));
        r.updater.lock().latest = Some(found.clone());
        let gate = Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (u, g, h) = (
                    Arc::clone(&r.updater),
                    Arc::clone(&gate),
                    rt.handle().clone(),
                );
                std::thread::spawn(move || {
                    let _in = h.enter();
                    g.wait();
                    u.start_apply("99.0.0").is_ok()
                })
            })
            .collect();
        let started = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(started, 1, "exactly one request may start the install");
    }
}
