//! MCP connections made beside the loop: the result of a connection task is applied by `App::on_mcp_connected`.

use super::*;

// REQ-AST-027, REQ-CFG-033: while the portal's new MCP server is still connecting, the loop rings a timer
// and applies a settings change; the server is added only when its result comes back. The server holds
// its reply until the test opens it, so nothing here depends on a sleep being long enough.
#[tokio::test(flavor = "current_thread")]
async fn a_timer_rings_and_a_setting_applies_while_an_mcp_connect_is_in_flight() {
    let dir = TempDir::new("mcp-in-flight");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let live = app.applied.clone();
    let mut done_rx = app.brain.connects.attach().unwrap();
    let (open_tx, open) = tokio::sync::watch::channel(false);
    let port = crate::connects::testutil::gated_server(open).await;
    app.brain
        .assistant
        .call_tool("set_timer", &serde_json::json!({"seconds": 1}))
        .await
        .unwrap();

    let (job, mut added) = ask(add_home(port));
    app.on_portal_job(job).await;
    assert!(app.brain.connects.is_connecting("home"));
    assert!(
        added.try_recv().is_err(),
        "the request waits for the server"
    );

    // The settings change goes through and is answered at once.
    let next = with(&live, &[("persona", "You are terse.".into())]);
    let (job, settings) = ask(Op::Settings(next));
    app.on_portal_job(job).await;
    assert!(settings.await.unwrap().is_ok());
    assert_eq!(app.brain.persona, "You are terse.");

    // The loop's tick rings the timer although the server has not answered.
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let rang = loop {
        tokio::select! {
            _ = tick.tick() => {
                if !app.brain.assistant.tick().is_empty() { break true; }
            }
            Some(done) = done_rx.recv() => {
                app.on_mcp_connected(done);
                break false;
            }
        }
    };
    assert!(rang, "the timer rang first");
    assert!(app.brain.connects.is_connecting("home"), "still connecting");
    assert!(!app.brain.mcp.has_server("home"), "nothing added yet");
    assert!(added.try_recv().is_err());

    // The server answers; the loop applies the result and answers the request.
    open_tx.send_replace(true);
    let done = done_rx.recv().await.unwrap();
    app.on_mcp_connected(done);
    let listing = added.await.unwrap().unwrap();
    assert!(app.brain.mcp.has_server("home") && app.brain.mcp.owns("home__ping"));
    assert!(!app.brain.connects.is_connecting("home"));
    assert!(listing.to_string().contains("home__ping"), "{listing}");
    assert_eq!(app.core.ui_tools(), listing);
}

// Removing a source that is still connecting stops it: the server never appears, not even when the server
// answers later, and the request that asked for it is told. The name can then be added again.
#[tokio::test(flavor = "current_thread")]
async fn removing_a_source_while_it_connects_cancels_it_and_it_can_be_added_again() {
    let dir = TempDir::new("mcp-cancel");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let mut done_rx = app.brain.connects.attach().unwrap();
    let (open_tx, open) = tokio::sync::watch::channel(false);
    let port = crate::connects::testutil::gated_server(open).await;

    let (job, first) = ask(add_home(port));
    app.on_portal_job(job).await;
    let (job, twin) = ask(add_home(port));
    app.on_portal_job(job).await;
    assert!(
        matches!(twin.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("already")),
        "a second connect for the name is refused"
    );

    let (job, removed) = ask(Op::Ext(ExtOp::McpDelete("home".into())));
    app.on_portal_job(job).await;
    assert!(removed.await.unwrap().is_ok());
    assert!(matches!(first.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("removed")));
    assert!(!app.brain.connects.is_connecting("home"));

    // The server answers after the removal: nothing arrives, nothing is added.
    open_tx.send_replace(true);
    let late = tokio::time::timeout(Duration::from_millis(300), done_rx.recv()).await;
    assert!(late.is_err(), "the cancelled task sent nothing");
    assert!(!app.brain.mcp.has_server("home"));

    // And the name is free again.
    let (job, again) = ask(add_home(port));
    app.on_portal_job(job).await;
    let done = done_rx.recv().await.unwrap();
    app.on_mcp_connected(done);
    assert!(again.await.unwrap().is_ok());
    assert!(app.brain.mcp.has_server("home"));
}

/// Save two connections in `dir` the way the portal would: `stuck` (answers only when told to, gives up
/// after 2 s) and `good`.
fn save_sources(dir: &TempDir, stuck_port: u16, good_port: u16) {
    crate::store::JsonStore::new(dir.0.join("connections.json"), crate::extensions::SCHEMA)
        .save(&serde_json::json!({ "servers": [
            { "name": "stuck", "url": format!("http://127.0.0.1:{stuck_port}/mcp"), "timeout_secs": 2 },
            { "name": "good", "url": format!("http://127.0.0.1:{good_port}/mcp") },
        ]}))
        .unwrap();
}

fn source<'a>(listing: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    listing["mcp"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no source {name} in {listing}"))
}

// ADR-0021, REQ-AST-027: saved sources connect after the loop is running. One that never answers does not
// hold up start-up: the loop rings a timer and answers a portal request while it is still connecting, the
// good source's tools appear as soon as it answers, and the stalled one ends as failed after its own
// timeout, still saved and listed with its reason.
#[tokio::test(flavor = "current_thread")]
async fn a_stalled_saved_source_does_not_hold_up_start_and_ends_failed_after_its_timeout() {
    let dir = TempDir::new("mcp-startup");
    let (_hold, closed) = tokio::sync::watch::channel(false);
    let (_open_tx, open) = tokio::sync::watch::channel(true);
    let stuck = crate::connects::testutil::gated_server(closed).await;
    let good = crate::connects::testutil::gated_server(open).await;
    save_sources(&dir, stuck, good);

    // Building the brain returns at once although one source will never answer.
    let began = std::time::Instant::now();
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let live = app.applied.clone();
    assert!(
        began.elapsed() < Duration::from_millis(1500),
        "start-up did not wait for the sources"
    );
    let mut done_rx = app.brain.connects.attach().unwrap();
    assert!(app.brain.mcp.is_connecting("stuck") && app.brain.mcp.is_connecting("good"));
    assert_eq!(app.brain.connects.len(), 2);
    assert_eq!(
        app.brain.connects.portal_len(),
        0,
        "saved sources are not portal requests"
    );
    let listing = app.brain.listing();
    assert_eq!(source(&listing, "stuck")["connecting"], true);
    assert_eq!(source(&listing, "stuck")["connected"], false);

    app.brain
        .assistant
        .call_tool("set_timer", &serde_json::json!({"seconds": 1}))
        .await
        .unwrap();

    // The loop: a portal request is answered at once, the good source is applied when it reports, and the
    // timer rings, all while the stalled source is still connecting.
    let next = with(&live, &[("persona", "You are terse.".into())]);
    let (job, settings) = ask(Op::Settings(next));
    app.on_portal_job(job).await;
    assert!(settings.await.unwrap().is_ok());
    assert_eq!(app.brain.persona, "You are terse.");

    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let (mut rang, mut good_in) = (false, false);
    while !(rang && good_in) {
        tokio::select! {
            _ = tick.tick() => {
                rang |= !app.brain.assistant.tick().is_empty();
            }
            Some(done) = done_rx.recv() => {
                assert_eq!(done.pending.name(), "good", "the stalled source cannot report first");
                app.on_mcp_connected(done);
                good_in = true;
            }
        }
    }
    assert!(
        app.brain.mcp.owns("good__ping"),
        "the good source's tools appear"
    );
    let listing = app.brain.listing();
    assert_eq!(source(&listing, "good")["connected"], true);
    assert_eq!(source(&listing, "good")["connecting"], false);
    assert!(app.brain.mcp.is_connecting("stuck"), "still connecting");
    assert_eq!(source(&listing, "stuck")["connecting"], true);
    assert_eq!(
        app.core.ui_tools(),
        listing,
        "the portal's copy is refreshed"
    );

    // Its own timeout ends the stalled one: failed, with the reason, and still saved.
    let done = done_rx.recv().await.unwrap();
    assert_eq!(done.pending.name(), "stuck");
    app.on_mcp_connected(done);
    let listing = app.brain.listing();
    let s = source(&listing, "stuck");
    assert_eq!(s["connected"], false);
    assert_eq!(s["connecting"], false);
    assert!(s["error"].as_str().is_some_and(|e| !e.is_empty()), "{s}");
    assert!(!app.brain.mcp.owns("stuck__ping") && app.brain.connects.is_empty());
    assert_eq!(listing["mcp"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        app.brain
            .ext
            .snapshot()
            .listing(&[], &app.brain.http_tools, &app.brain.mcp)["mcp"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "both stay saved"
    );
}

// Removing a saved source that is still connecting removes it and stops its connection; its late result
// is not applied.
#[tokio::test(flavor = "current_thread")]
async fn removing_a_saved_source_while_it_connects_stops_it() {
    let dir = TempDir::new("mcp-startup-remove");
    let (open_tx, closed) = tokio::sync::watch::channel(false);
    let (_g, open) = tokio::sync::watch::channel(true);
    let stuck = crate::connects::testutil::gated_server(closed).await;
    let good = crate::connects::testutil::gated_server(open).await;
    save_sources(&dir, stuck, good);
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let mut done_rx = app.brain.connects.attach().unwrap();
    assert!(app.brain.connects.is_connecting("stuck"));

    let (job, removed) = ask(Op::Ext(ExtOp::McpDelete("stuck".into())));
    app.on_portal_job(job).await;
    assert!(removed.await.unwrap().is_ok());
    assert!(!app.brain.connects.is_connecting("stuck") && !app.brain.mcp.has_server("stuck"));

    // Even if the server answers now, nothing arrives for the removed name.
    open_tx.send_replace(true);
    let done = done_rx.recv().await.unwrap();
    assert_eq!(done.pending.name(), "good");
    app.on_mcp_connected(done);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), done_rx.recv())
            .await
            .is_err()
    );
    assert!(!app.brain.mcp.has_server("stuck") && app.brain.mcp.owns("good__ping"));
}

// A connection task that dies is answered like any other failed connection, and the name can be tried
// again.
#[tokio::test(flavor = "current_thread")]
async fn a_connect_task_that_dies_answers_the_request_and_frees_the_name() {
    let dir = TempDir::new("mcp-died");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let mut done_rx = app.brain.connects.attach().unwrap();
    app.brain.connects.make_next_task_die();
    let (job, answer) = ask(add_home(9));
    app.on_portal_job(job).await;
    let done = done_rx.recv().await.unwrap();
    app.on_mcp_connected(done);
    assert!(matches!(
        answer.await.unwrap(),
        Err(Fail::Invalid(m)) if m == "The connection could not be made."
    ));
    assert!(!app.brain.mcp.has_server("home") && app.brain.connects.is_empty());
    let (job, _again) = ask(add_home(9));
    app.on_portal_job(job).await;
    assert!(app.brain.connects.is_connecting("home"));
}

// A server that cannot be used is reported to the request and leaves nothing behind.
#[tokio::test(flavor = "current_thread")]
async fn a_connect_that_fails_answers_the_request_and_adds_nothing() {
    let dir = TempDir::new("mcp-fail");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let mut done_rx = app.brain.connects.attach().unwrap();
    let (job, answer) = ask(add_home(9));
    app.on_portal_job(job).await;
    let done = done_rx.recv().await.unwrap();
    app.on_mcp_connected(done);
    assert!(matches!(answer.await.unwrap(), Err(Fail::Invalid(_))));
    assert!(!app.brain.mcp.has_server("home") && app.brain.connects.is_empty());
    // A bad request is refused on the spot, without a task.
    let (job, bad) = ask(Op::Ext(ExtOp::McpAdd(
        serde_json::json!({"name": "Bad Name", "url": "http://h/mcp"}),
    )));
    app.on_portal_job(job).await;
    assert!(matches!(bad.await.unwrap(), Err(Fail::Invalid(_))));
    assert!(app.brain.connects.is_empty());
}
