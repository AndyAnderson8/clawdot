//! The administrator reset request (REQ-SAT-020, REQ-DEV-031).

use super::*;

async fn next_reset_answer(p: &mut Peer) -> (u32, Option<String>) {
    loop {
        match p.recv().await {
            Message::AdminResetWindow { open_secs, reason } => return (open_secs, reason),
            Message::Ping => p.send(&Message::Pong).await,
            _ => {}
        }
    }
}

fn counting_hook(calls: &Arc<AtomicU64>, answer: Result<u32, String>) -> AdminResetHook {
    let calls = Arc::clone(calls);
    AdminResetHook::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        answer.clone()
    })
}

async fn welcomed(s: Settings, token: Option<&str>) -> Peer {
    let (mut p, h) = start(s, "x", "y");
    std::mem::forget(h);
    p.hello(token, PROTOCOL_VERSION).await;
    assert!(matches!(p.recv().await, Message::Welcome(_)));
    p
}

#[tokio::test]
async fn a_loopback_satellite_can_open_the_reset_window() {
    let calls = Arc::new(AtomicU64::new(0));
    let mut s = fast();
    s.peer_loopback = true;
    s.admin_reset = Some(counting_hook(&calls, Ok(300)));
    let mut p = welcomed(s, None).await;
    p.send(&Message::AdminReset).await;
    assert_eq!(next_reset_answer(&mut p).await, (300, None));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_network_satellite_without_a_token_is_refused_and_the_core_is_not_asked() {
    let calls = Arc::new(AtomicU64::new(0));
    let mut s = fast();
    s.admin_reset = Some(counting_hook(&calls, Ok(300)));
    let mut p = welcomed(s, None).await;
    p.send(&Message::AdminReset).await;
    let (secs, reason) = next_reset_answer(&mut p).await;
    assert_eq!(secs, 0);
    assert!(reason.unwrap().contains("not trusted"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a guest on the LAN gets nothing"
    );
}

#[tokio::test]
async fn a_network_satellite_that_proved_a_token_is_trusted() {
    let calls = Arc::new(AtomicU64::new(0));
    let mut s = fast();
    s.token_hashes = vec![hash_token("sat-secret")];
    s.admin_reset = Some(counting_hook(&calls, Ok(300)));
    let mut p = welcomed(s, Some("sat-secret")).await;
    p.send(&Message::AdminReset).await;
    assert_eq!(next_reset_answer(&mut p).await.0, 300);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_refusal_from_the_core_and_a_missing_hook_are_both_explained() {
    let calls = Arc::new(AtomicU64::new(0));
    let mut s = fast();
    s.peer_loopback = true;
    s.admin_reset = Some(counting_hook(
        &calls,
        Err("Wait a minute and try again.".into()),
    ));
    let mut p = welcomed(s, None).await;
    p.send(&Message::AdminReset).await;
    assert_eq!(
        next_reset_answer(&mut p).await,
        (0, Some("Wait a minute and try again.".into()))
    );
    let mut s = fast();
    s.peer_loopback = true;
    let mut p = welcomed(s, None).await;
    p.send(&Message::AdminReset).await;
    assert_eq!(next_reset_answer(&mut p).await.0, 0);
}

#[tokio::test]
async fn a_satellite_cannot_hammer_the_reset_request() {
    let calls = Arc::new(AtomicU64::new(0));
    let mut s = fast();
    s.peer_loopback = true;
    s.admin_reset = Some(counting_hook(&calls, Ok(300)));
    let mut p = welcomed(s, None).await;
    for _ in 0..5 {
        p.send(&Message::AdminReset).await;
    }
    let mut answers = Vec::new();
    for _ in 0..5 {
        answers.push(next_reset_answer(&mut p).await.0);
    }
    assert_eq!(answers, [300, 0, 0, 0, 0]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
