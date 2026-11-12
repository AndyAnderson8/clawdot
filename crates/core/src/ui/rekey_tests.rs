//! A changed key is saved even when the running assistant cannot take it; the page is told the truth.
#![allow(clippy::unwrap_used)]

use super::tests::{call, start_in};

#[tokio::test]
async fn a_key_the_running_assistant_could_not_take_is_reported_as_saved_but_not_in_use() {
    let dir = std::env::temp_dir().join(format!("clawdot-rekey-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // No main loop is listening, as when it is too busy to take the request.
    let p = start_in(Some(&dir), false).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());

    let put = r#"{"key":"sk-test-0123456789abcdef0123"}"#;
    let (s, v) = call(&p, "PUT", "/api/llm/key", &session, put).await;
    assert_eq!(s, 503, "{v}");
    let msg = v["error"].as_str().unwrap();
    assert!(msg.contains("key is saved"), "{msg}");
    assert!(msg.contains("after a restart"), "{msg}");
    assert!(!v.to_string().contains("0123456789abcdef"));
    // It really is saved.
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["llm"]["key_hint"], "sk-tes…0123");

    let (s, v) = call(&p, "DELETE", "/api/llm/key", &session, "").await;
    assert_eq!(s, 503, "{v}");
    assert!(v["error"].as_str().unwrap().contains("key is removed"));
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert!(v["llm"]["key_hint"].is_null() || v["llm"]["key_hint"] == "");
    let _ = std::fs::remove_dir_all(&dir);
}
