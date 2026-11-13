use super::*;

#[tokio::test(flavor = "current_thread")]
async fn streams_require_credentials_and_are_capped() {
    let h = start(
        Limits::default(),
        ServeOptions {
            max_streams: 1,
            ..ServeOptions::default()
        },
    )
    .await;
    assert_eq!(
        call(h.addr, "GET", "/bridge/claude-code/replies", &[], "")
            .await
            .0,
        401
    );
    let mut first = open_stream(h.addr, &h.token).await;
    let mut buf = String::new();
    read_until(&mut first, &mut buf, "retry:").await;
    let auth = bearer(&h.token);
    let (status, _, _) = call(
        h.addr,
        "GET",
        "/bridge/claude-code/replies",
        &[("Authorization", &auth)],
        "",
    )
    .await;
    assert_eq!(status, 503, "the second stream is refused");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stream_whose_reader_stopped_reading_ends_and_frees_its_slot() {
    let core = Arc::new(Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    ));
    let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&bearer(&token)).unwrap(),
    );
    let streams = Arc::new(AtomicUsize::new(0));
    let opts = ServeOptions {
        heartbeat: Duration::from_millis(5),
        write_timeout: Duration::from_millis(50),
        ..ServeOptions::default()
    };
    // The response body is held but never polled: a client that has stopped reading.
    let response = replies_stream(
        core,
        Arc::clone(&streams),
        opts,
        &headers,
        LOCALHOST_PEER,
        "claude-code",
    );
    assert_eq!(response.status(), 200);
    assert_eq!(streams.load(Ordering::SeqCst), 1);
    let freed = timeout(Duration::from_secs(5), async {
        while streams.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(freed.is_ok(), "the stalled stream must release its slot");
    drop(response);
}

#[tokio::test(flavor = "current_thread")]
async fn rotating_the_token_ends_the_stream() {
    let h = start(
        Limits::default(),
        ServeOptions {
            heartbeat: Duration::from_millis(100),
            ..ServeOptions::default()
        },
    )
    .await;
    let mut s = open_stream(h.addr, &h.token).await;
    let mut buf = String::new();
    read_until(&mut s, &mut buf, "retry:").await;
    h.core.rotate_endpoint("claude-code").unwrap();
    // The next heartbeat re-checks credentials and the server closes the stream.
    let mut chunk = [0_u8; 256];
    let closed = timeout(Duration::from_secs(5), async {
        loop {
            match s.read(&mut chunk).await {
                Ok(0) | Err(_) => return true,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert_eq!(closed, Ok(true));
}
