use super::*;

#[tokio::test(flavor = "current_thread")]
async fn oversized_bodies_are_refused_without_being_read() {
    let limits = Limits {
        max_body: 1024,
        ..Limits::default()
    };
    let h = start(limits, ServeOptions::default()).await;
    let auth = bearer(&h.token);
    // Declares 10 MB but sends almost nothing: the server must answer 413 immediately.
    let mut s = TcpStream::connect(h.addr).await.unwrap();
    let req = format!(
        "POST /hook/claude-code HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {auth}\r\nContent-Length: 10485760\r\nConnection: close\r\n\r\n{{}}"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 413"));
    // And an actually-large body is cut off at the limit.
    let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(5000));
    let (status, _, _) = call(
        h.addr,
        "POST",
        "/hook/claude-code",
        &[("Authorization", &auth)],
        &big,
    )
    .await;
    assert_eq!(status, 413);
}

#[tokio::test(flavor = "current_thread")]
async fn a_flood_gets_429s() {
    let h = start(
        Limits {
            hook_burst: 3.0,
            hook_per_sec: 0.1,
            ..Limits::default()
        },
        ServeOptions::default(),
    )
    .await;
    let auth = bearer(&h.token);
    let mut seen_429 = false;
    for i in 0..10 {
        let body = format!(r#"{{"hook_event_name":"Stop","session_id":"s{i}","cwd":"/a/b"}}"#);
        let (status, head, _) = call(
            h.addr,
            "POST",
            "/hook/claude-code",
            &[("Authorization", &auth)],
            &body,
        )
        .await;
        if status == 429 {
            assert!(head.to_lowercase().contains("retry-after"));
            seen_429 = true;
        }
    }
    assert!(seen_429);
}

/// True when the server closes `s` (end of stream or reset) within `ms` milliseconds.
async fn closed_within(s: &mut TcpStream, ms: u64) -> bool {
    let mut buf = [0_u8; 512];
    matches!(
        timeout(Duration::from_millis(ms), async {
            loop {
                match s.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
            }
        })
        .await,
        Ok(())
    )
}

#[tokio::test(flavor = "current_thread")]
async fn idle_sockets_cannot_hold_slots_one_address_cannot_take_them_all_and_they_time_out() {
    let h = start(
        Limits::default(),
        ServeOptions {
            max_per_ip: 2,
            header_timeout: Duration::from_millis(300),
            ..ServeOptions::default()
        },
    )
    .await;
    // Two idle sockets use this address's share; the third is refused at once.
    let mut a = TcpStream::connect(h.addr).await.unwrap();
    let mut b = TcpStream::connect(h.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut c = TcpStream::connect(h.addr).await.unwrap();
    assert!(closed_within(&mut c, 200).await, "over the per-address cap");
    // The idle ones are dropped by the header timeout, freeing the share again.
    assert!(closed_within(&mut a, 2_000).await && closed_within(&mut b, 2_000).await);
    let (status, _, _) = call(h.addr, "GET", "/healthz", &[], "").await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stalled_request_body_is_cut_off() {
    let h = start(
        Limits::default(),
        ServeOptions {
            body_timeout: Duration::from_millis(300),
            ..ServeOptions::default()
        },
    )
    .await;
    let mut s = TcpStream::connect(h.addr).await.unwrap();
    let head = format!(
        "POST /hook/claude-code HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nContent-Length: 1000\r\n\r\n{{\"a\":",
        h.token
    );
    s.write_all(head.as_bytes()).await.unwrap();
    // The server answers (413) and closes instead of waiting for the other 994 bytes.
    let mut got = Vec::new();
    let done = timeout(Duration::from_secs(3), s.read_to_end(&mut got)).await;
    assert!(done.is_ok(), "the stalled body kept the connection open");
    assert!(String::from_utf8_lossy(&got).contains("413"));
    assert!(
        h.core
            .bridge_pending("claude-code", &h.token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
}
