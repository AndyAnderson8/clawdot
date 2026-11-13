use super::*;

#[tokio::test(flavor = "current_thread")]
async fn a_claude_code_hook_is_accepted_and_the_response_is_an_empty_object() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let auth = bearer(&h.token);
    let (status, head, body) = call(
        h.addr,
        "POST",
        "/hook/claude-code",
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
        ],
        STOP,
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(
        body, "{}",
        "hook runtimes parse the body, so it must carry no fields"
    );
    assert!(
        head.to_lowercase()
            .contains("x-clawdot-event: accepted; id=1"),
        "{head}"
    );
    assert_eq!(h.core.events().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_accepted_event_is_on_disk_before_the_response_arrives() {
    let dir = crate::store::testutil::TempDir::new("http-durable");
    let core = Arc::new(
        Core::open(
            Arc::new(SystemClock),
            Adapters::builtin(),
            Limits::default(),
            crate::app::Stores::in_dir(&dir.0),
        )
        .unwrap(),
    );
    let h = start_core(core, ServeOptions::default()).await;
    let auth = bearer(&h.token);
    let (status, _, _) = call(
        h.addr,
        "POST",
        "/hook/claude-code",
        &[("Authorization", &auth)],
        STOP,
    )
    .await;
    assert_eq!(status, 202);
    // Read at once, with no flush: the response itself must have waited for the write (REQ-INB-004).
    let saved = std::fs::read_to_string(dir.0.join("inbox.json")).unwrap();
    assert!(saved.contains("clawdot finished"), "{saved}");
}

async fn queue_reply(h: &Harness, text: &str) {
    let auth = bearer(&h.token);
    call(
        h.addr,
        "POST",
        "/hook/claude-code",
        &[("Authorization", &auth)],
        STOP,
    )
    .await;
    let ev = h.core.next_announcement(true, 0).unwrap().event;
    h.core.mark_announced(&[ev.id]);
    h.core.reply(ReplyKind::Instruct, text).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_bridge_receives_a_reply_queued_before_and_after_it_connects() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    queue_reply(&h, "keep going").await; // queued before connecting

    let mut s = open_stream(h.addr, &h.token).await;
    let mut buf = String::new();
    read_until(&mut s, &mut buf, "event: reply").await;
    assert!(buf.starts_with("HTTP/1.1 200") && buf.to_lowercase().contains("text/event-stream"));
    assert!(
        buf.contains("\"text\":\"keep going\"") && buf.contains("\"session_ref\":\"s1\""),
        "{buf}"
    );

    // A reply queued while the stream is open is pushed promptly.
    let auth = bearer(&h.token);
    let second = r#"{"session_id":"s2","cwd":"/a/b","hook_event_name":"Stop"}"#;
    call(
        h.addr,
        "POST",
        "/hook/claude-code",
        &[("Authorization", &auth)],
        second,
    )
    .await;
    let ev = h.core.next_announcement(true, 0).unwrap().event;
    h.core.mark_announced(&[ev.id]);
    h.core.reply(ReplyKind::Instruct, "and now this").unwrap();
    read_until(&mut s, &mut buf, "and now this").await;
}

#[tokio::test(flavor = "current_thread")]
async fn unacknowledged_replies_are_resent_on_reconnect_and_acks_clear_them() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    queue_reply(&h, "do the thing").await;
    {
        let mut s = open_stream(h.addr, &h.token).await;
        let mut buf = String::new();
        read_until(&mut s, &mut buf, "do the thing").await;
    } // connection dropped without acknowledging

    let mut s = open_stream(h.addr, &h.token).await;
    let mut buf = String::new();
    read_until(&mut s, &mut buf, "do the thing").await; // resent

    let auth = bearer(&h.token);
    let (status, _, _) = call(
        h.addr,
        "POST",
        "/bridge/claude-code/replies/1/ack",
        &[("Authorization", &auth)],
        "",
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(
        call(
            h.addr,
            "POST",
            "/bridge/claude-code/replies/1/ack",
            &[("Authorization", &auth)],
            ""
        )
        .await
        .0,
        404
    );
    assert!(
        h.core
            .bridge_pending("claude-code", &h.token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
}

/// A core with its files in `dir`, served, with one endpoint (`claude-code`).
async fn start_stored(dir: &crate::store::testutil::TempDir) -> Harness {
    let core = Arc::new(
        Core::open(
            Arc::new(SystemClock),
            Adapters::builtin(),
            Limits::default(),
            crate::app::Stores::in_dir(&dir.0),
        )
        .unwrap(),
    );
    start_core(core, ServeOptions::default()).await
}

/// Make every later save of `name` in `dir` fail: a directory sits where its temporary file would be written.
fn break_saves(dir: &crate::store::testutil::TempDir, name: &str) -> std::path::PathBuf {
    let blocker = crate::store::temp_path(&dir.0.join(name));
    std::fs::create_dir(&blocker).unwrap();
    blocker
}

// REQ-INB-004: a webhook is answered 202 only if it was saved. When it could not be, the answer is a 5xx with a
// plain message (the event is still held in memory and announced), and the next one that can be saved is a 202.
#[tokio::test(flavor = "current_thread")]
async fn a_webhook_that_could_not_be_saved_is_not_answered_202() {
    let dir = crate::store::testutil::TempDir::new("hook-unsaved");
    let h = start_stored(&dir).await;
    let auth = bearer(&h.token);
    let hdr = [("Authorization", auth.as_str())];
    h.core.flush_blocking();
    let blocker = break_saves(&dir, "inbox.json");
    let (status, _, body) = call(h.addr, "POST", "/hook/claude-code", &hdr, STOP).await;
    assert_eq!(status, 500, "{body}");
    assert!(
        json_of(&body)["error"]
            .as_str()
            .unwrap()
            .contains("could not be saved")
    );
    assert_eq!(
        h.core.events().len(),
        1,
        "the event is kept and will be announced"
    );
    std::fs::remove_dir(&blocker).unwrap();
    let other = STOP.replace("s1", "s2");
    let (status, ..) = call(h.addr, "POST", "/hook/claude-code", &hdr, &other).await;
    assert_eq!(status, 202, "saving works again");
    let (status, ..) = call(h.addr, "GET", "/healthz", &[], "").await;
    assert_eq!(status, 200, "requests that save nothing are unaffected");
}

// The same for an announcement, a bridge acknowledgement and an endpoint made in the portal.
#[tokio::test(flavor = "current_thread")]
async fn announcements_acks_and_portal_endpoints_that_could_not_be_saved_are_5xx() {
    let dir = crate::store::testutil::TempDir::new("others-unsaved");
    let h = start_stored(&dir).await;
    let auth = bearer(&h.token);
    let hdr = [("Authorization", auth.as_str())];
    let session = admin(&h);
    let portal = [("Authorization", session.as_str())];

    // Announcement.
    let blocker = break_saves(&dir, "inbox.json");
    let say = r#"{"text":"hello"}"#;
    let (status, ..) = call(h.addr, "POST", "/api/announce", &hdr, say).await;
    assert_eq!(status, 500);
    std::fs::remove_dir(&blocker).unwrap();
    let again = r#"{"text":"hello again"}"#;
    let (status, ..) = call(h.addr, "POST", "/api/announce", &hdr, again).await;
    assert_eq!(status, 202);

    // Acknowledgement of a reply.
    h.core
        .ingest_hook("claude-code", &h.token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    let id = h.core.events().last().unwrap().id;
    h.core.mark_announced(&[id]);
    h.core
        .reply(crate::inbox::reply::ReplyKind::Instruct, "go on")
        .unwrap();
    h.core.flush_blocking();
    let reply = h
        .core
        .bridge_pending("claude-code", &h.token, LOCALHOST_PEER, 0)
        .unwrap()[0]
        .id;
    let blocker = break_saves(&dir, "replies.json");
    let path = format!("/bridge/claude-code/replies/{reply}/ack");
    let (status, ..) = call(h.addr, "POST", &path, &hdr, "").await;
    assert_eq!(status, 500, "an acknowledgement that was not saved");
    std::fs::remove_dir(&blocker).unwrap();

    // An endpoint made in the portal is taken back, so that trying again works.
    let blocker = break_saves(&dir, "endpoints.json");
    let new = r#"{"name":"second","adapter":"generic"}"#;
    let (status, _, body) = call(h.addr, "POST", "/api/endpoints", &portal, new).await;
    assert_eq!(status, 500, "{body}");
    assert!(h.core.endpoints().iter().all(|e| e.name != "second"));
    std::fs::remove_dir(&blocker).unwrap();
    let (status, ..) = call(h.addr, "POST", "/api/endpoints", &portal, new).await;
    assert_eq!(status, 201);
}
