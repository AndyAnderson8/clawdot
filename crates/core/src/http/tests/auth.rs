use super::*;

/// A clock that never moves, so token-bucket refill cannot depend on how fast the machine is.
struct FrozenClock;
impl crate::sched::Clock for FrozenClock {
    fn monotonic_secs(&self) -> i64 {
        1_800_000_000
    }
    fn now(&self) -> crate::sched::Ts {
        1_800_000_000
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_alternate_token_header_works() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let (status, _, _) = call(
        h.addr,
        "POST",
        "/hook/claude-code",
        &[("X-ClawDot-Token", &h.token)],
        STOP,
    )
    .await;
    assert_eq!(status, 202);
}

#[tokio::test(flavor = "current_thread")]
async fn missing_wrong_and_unknown_credentials_all_get_401() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let wrong = bearer("cdt_wrong");
    for (path, headers) in [
        ("/hook/claude-code", vec![]),
        ("/hook/claude-code", vec![("Authorization", wrong.as_str())]),
        ("/hook/unknown", vec![("Authorization", wrong.as_str())]),
    ] {
        let (status, head, _) = call(h.addr, "POST", path, &headers, STOP).await;
        assert_eq!(status, 401, "{path}");
        assert!(head.to_lowercase().contains("www-authenticate: bearer"));
    }
    assert!(h.core.events().is_empty());
}

#[tokio::test]
async fn the_api_needs_a_signed_in_administrator() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    // Setup not done yet: everyone is refused (REQ-CFG-018).
    assert_eq!(call(h.addr, "GET", "/api/overview", &[], "").await.0, 403);
    let good = admin(&h);
    let bad = bearer("nope");
    assert_eq!(call(h.addr, "GET", "/api/overview", &[], "").await.0, 401);
    assert_eq!(
        call(
            h.addr,
            "GET",
            "/api/overview",
            &[("Authorization", bad.as_str())],
            ""
        )
        .await
        .0,
        401
    );
    // An endpoint token is not an administrator token.
    let ep = bearer(&h.token);
    assert_eq!(
        call(
            h.addr,
            "GET",
            "/api/overview",
            &[("Authorization", ep.as_str())],
            ""
        )
        .await
        .0,
        401
    );
    let (status, _, body) = call(
        h.addr,
        "GET",
        "/api/overview",
        &[("Authorization", good.as_str())],
        "",
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.contains("claude-code"));
    assert!(!body.contains("token_hash"), "hashes are never shown");
}

#[tokio::test]
async fn wrong_administrator_guesses_get_rate_limited() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    admin(&h);
    let bad = bearer("guess");
    let mut limited = false;
    for _ in 0..200 {
        let (status, ..) = call(
            h.addr,
            "GET",
            "/api/overview",
            &[("Authorization", bad.as_str())],
            "",
        )
        .await;
        if status == 429 {
            limited = true;
            break;
        }
    }
    assert!(limited, "guessing must be throttled");
}

async fn get_status(h: &Harness, path: &str, auth: &str) -> u16 {
    let hdrs: Vec<(&str, &str)> = if auth.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization", auth)]
    };
    call(h.addr, "GET", path, &hdrs, "").await.0
}

fn session_of(v: &serde_json::Value) -> String {
    format!("Bearer {}", v["token"].as_str().unwrap())
}

// REQ-CFG-018
#[tokio::test]
async fn first_run_offers_setup_and_refuses_everything_else() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let (status, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(status, 200);
    let v = json_of(&body);
    assert_eq!(v["mode"], "setup");
    assert_eq!(v["open_allowed"], true);
    assert_eq!(v["peer_private"], true);
    // Nothing but setup works yet, with or without credentials.
    for (m, p) in [
        ("GET", "/api/overview"),
        ("GET", "/api/status"),
        ("POST", "/api/speak"),
        ("POST", "/api/admin/password"),
    ] {
        assert_eq!(call(h.addr, m, p, &[], "{}").await.0, 403, "{p}");
    }
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"x"}"#).await.0,
        403
    );
    // No token wording is left in the page, and it can show the banner (REQ-CFG-018, REQ-CFG-020).
    let (_, _, page) = call(h.addr, "GET", "/", &[], "").await;
    assert!(!page.contains("Administrator token"), "old wording is gone");
    assert!(page.contains("\"banner\"") && page.contains("Unprotected"));
}

// REQ-CFG-018, REQ-CFG-019, REQ-CFG-024
#[tokio::test]
async fn a_password_signs_in_for_a_session_and_can_be_changed() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    for bad in [
        r#"{"mode":"password"}"#,
        r#"{"mode":"password","password":"short"}"#,
        r#"{"mode":"nope"}"#,
        r#"{"mode":"password","password":"long enough","extra":1}"#,
    ] {
        assert_eq!(post(&h, "/api/setup", "", bad).await.0, 400, "{bad}");
    }
    assert_eq!(h.core.admin_state()["mode"], "setup");
    let (status, v) = post(
        &h,
        "/api/setup",
        "",
        r#"{"mode":"password","password":"first password"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let s1 = session_of(&v);
    // Setup cannot be run again by anyone.
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        409
    );
    assert_eq!(h.core.admin_state()["mode"], "password");
    // The session works; the password itself is not a credential for the API.
    assert_eq!(get_status(&h, "/api/overview", &s1).await, 200);
    assert_eq!(
        get_status(&h, "/api/overview", &bearer("first password")).await,
        401
    );
    // Sign in again: wrong, then right.
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"first passwore"}"#)
            .await
            .0,
        401
    );
    let (status, v) = post(&h, "/api/login", "", r#"{"password":"first password"}"#).await;
    assert_eq!(status, 200);
    let s2 = session_of(&v);
    // Sign out ends just that session.
    assert_eq!(post(&h, "/api/logout", &s2, "").await.0, 200);
    assert_eq!(get_status(&h, "/api/overview", &s2).await, 401);
    assert_eq!(get_status(&h, "/api/overview", &s1).await, 200);
    // Changing the password needs a session, checks length, and ends the others.
    assert_eq!(
        post(
            &h,
            "/api/admin/password",
            "",
            r#"{"password":"another one"}"#
        )
        .await
        .0,
        401
    );
    assert_eq!(
        post(&h, "/api/admin/password", &s1, r#"{"password":"tiny"}"#)
            .await
            .0,
        400
    );
    // The current password must be given, and be right (REQ-CFG-024).
    for body in [
        r#"{"password":"another one"}"#,
        r#"{"password":"another one","current":"not the password"}"#,
    ] {
        let (status, v) = post(&h, "/api/admin/password", &s1, body).await;
        assert_eq!(status, 403, "{body}");
        assert!(v["error"].as_str().unwrap().contains("current password"));
    }
    assert_eq!(
        get_status(&h, "/api/overview", &s1).await,
        200,
        "a wrong current password does not end the session"
    );
    let (status, v) = post(
        &h,
        "/api/admin/password",
        &s1,
        r#"{"password":"another one","current":"first password"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let s3 = session_of(&v);
    assert_eq!(get_status(&h, "/api/overview", &s1).await, 401);
    assert_eq!(get_status(&h, "/api/overview", &s3).await, 200);
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"first password"}"#)
            .await
            .0,
        401
    );
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"another one"}"#)
            .await
            .0,
        200
    );
}

// REQ-CFG-020, REQ-CFG-024, REQ-INB-002
#[tokio::test]
async fn open_mode_needs_no_sign_in_but_webhooks_still_need_tokens() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["mode"], "open");
    assert_eq!(get_status(&h, "/api/overview", "").await, 200);
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"anything"}"#)
            .await
            .0,
        409,
        "there is no password to sign in with"
    );
    // Endpoint tokens stay required (REQ-INB-002).
    assert_eq!(
        call(h.addr, "POST", "/hook/claude-code", &[], STOP).await.0,
        401
    );
    let bad = bearer("cdt_wrong");
    let hdr = [("Authorization", bad.as_str())];
    assert_eq!(
        call(h.addr, "POST", "/hook/claude-code", &hdr, STOP)
            .await
            .0,
        401
    );
    let good = bearer(&h.token);
    let hdr = [("Authorization", good.as_str())];
    assert_eq!(
        call(h.addr, "POST", "/hook/claude-code", &hdr, STOP)
            .await
            .0,
        202
    );
    // From open mode, a password can be set without signing in first.
    let (status, v) = post(
        &h,
        "/api/admin/password",
        "",
        r#"{"password":"now protected"}"#,
    )
    .await;
    assert_eq!(status, 200);
    assert!(v["token"].is_string());
    assert_eq!(get_status(&h, "/api/overview", "").await, 401);
}

// REQ-CFG-021
#[tokio::test]
async fn setup_needs_no_code_when_listening_beyond_loopback() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    h.core.set_listen_ip([192, 168, 1, 20].into());
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert!(json_of(&body).get("code_needed").is_none());
    // A stale client that still sends a code is refused as a malformed request.
    let old = r#"{"mode":"open","code":"0000-0000"}"#;
    assert_eq!(post(&h, "/api/setup", "", old).await.0, 400);
    assert_eq!(h.core.admin_state()["mode"], "setup");
    let ok = r#"{"mode":"password","password":"no code needed"}"#;
    assert_eq!(post(&h, "/api/setup", "", ok).await.0, 200);
    assert_eq!(h.core.admin_state()["mode"], "password");
}

// REQ-CFG-020
#[tokio::test]
async fn open_mode_is_refused_on_a_public_listen_address() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    h.core.set_listen_ip([203, 0, 113, 9].into());
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["open_allowed"], false);
    let open = r#"{"mode":"open"}"#;
    assert_eq!(post(&h, "/api/setup", "", open).await.0, 403);
    let pw = r#"{"mode":"password","password":"still fine"}"#;
    assert_eq!(post(&h, "/api/setup", "", pw).await.0, 200);
}

// REQ-CFG-019
#[tokio::test]
async fn sign_in_attempts_are_rate_limited() {
    // A frozen clock: the bucket cannot refill however slow each (PBKDF2) attempt is, so the burst is
    // spent after exactly `auth_fail_burst` attempts on any machine.
    let limits = Limits::default();
    let burst = limits.auth_fail_burst as usize;
    let h = start_with_clock(Arc::new(FrozenClock), limits, ServeOptions::default()).await;
    admin(&h);
    for i in 0..burst {
        let (status, ..) = post(&h, "/api/login", "", r#"{"password":"guess guess"}"#).await;
        assert_eq!(status, 401, "attempt {i} is within the burst");
    }
    let (status, ..) = post(&h, "/api/login", "", r#"{"password":"guess guess"}"#).await;
    assert_eq!(status, 429, "password guessing must be throttled");
}

// REQ-CFG-023
#[tokio::test]
async fn an_old_administrator_token_signs_in_like_a_password() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let old = crate::inbox::endpoint::generate_token().unwrap();
    let hash = crate::inbox::endpoint::hash_token(&old);
    h.core
        .admin_setup_password([127, 0, 0, 1].into(), hash)
        .unwrap();
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    let v = json_of(&body);
    assert_eq!(v["mode"], "password");
    assert_eq!(v["legacy"], true);
    let login = format!(r#"{{"password":"{old}"}}"#);
    let (status, v) = post(&h, "/api/login", "", &login).await;
    assert_eq!(status, 200);
    let s = session_of(&v);
    let new = format!(r#"{{"password":"a real password","current":"{old}"}}"#);
    assert_eq!(post(&h, "/api/admin/password", &s, &new).await.0, 200);
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["legacy"], false);
    assert_eq!(
        post(&h, "/api/login", "", &login).await.0,
        401,
        "the old token stops working once replaced"
    );
}

// REQ-SEC-004: failed attempts are counted per source address. Address A guessing until it is refused leaves
// address B able to present the right token and the administrator able to sign in with the right password.
#[tokio::test(flavor = "current_thread")]
async fn one_clients_failures_do_not_lock_out_another_client() {
    let h = start_with_clock(
        Arc::new(FrozenClock),
        Limits::default(),
        ServeOptions::default(),
    )
    .await;
    admin(&h);
    let (a, b) = ("127.0.0.2", "127.0.0.3");
    let wrong = bearer("cdt_wrong");
    let right = bearer(&h.token);
    let burst = Limits::default().auth_fail_burst as usize;
    async fn from(h: &Harness, who: &str, path: &str, auth: &str, body: &str) -> u16 {
        let hdrs = [("Authorization", auth)];
        let hdrs: &[(&str, &str)] = if auth.is_empty() { &[] } else { &hdrs };
        call_from(who, h.addr, "127.0.0.1", "POST", path, hdrs, body)
            .await
            .0
    }
    for i in 0..burst {
        let status = from(&h, a, "/hook/claude-code", &wrong, STOP).await;
        assert_eq!(status, 401, "attempt {i}");
    }
    let password = r#"{"password":"adm-secret"}"#;
    assert_eq!(from(&h, a, "/hook/claude-code", &wrong, STOP).await, 429);
    assert_eq!(
        from(&h, a, "/api/login", "", password).await,
        429,
        "A is throttled"
    );
    assert_eq!(
        from(&h, b, "/hook/claude-code", &right, STOP).await,
        202,
        "B's token works"
    );
    assert_eq!(
        from(&h, b, "/api/login", "", password).await,
        200,
        "B signs in"
    );
    assert_eq!(
        from(&h, b, "/hook/claude-code", &wrong, STOP).await,
        401,
        "B is still within its own budget"
    );
}
