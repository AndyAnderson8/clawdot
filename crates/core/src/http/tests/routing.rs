use super::*;

#[tokio::test(flavor = "current_thread")]
async fn health_check_works_without_credentials() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let (status, _, body) = call(h.addr, "GET", "/healthz", &[], "").await;
    assert_eq!((status, body.as_str()), (200, r#"{"status":"ok"}"#));
}

#[tokio::test(flavor = "current_thread")]
async fn bad_json_unknown_paths_and_wrong_methods_get_the_right_statuses() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let auth = bearer(&h.token);
    assert_eq!(
        call(
            h.addr,
            "POST",
            "/hook/claude-code",
            &[("Authorization", &auth)],
            "not json"
        )
        .await
        .0,
        400
    );
    assert_eq!(
        call(h.addr, "GET", "/hook/claude-code", &[], "").await.0,
        405
    );
    assert_eq!(call(h.addr, "GET", "/nope", &[], "").await.0, 404);
    assert_eq!(
        call(
            h.addr,
            "POST",
            "/hook/..%2f..",
            &[("Authorization", &auth)],
            "{}"
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(
            h.addr,
            "POST",
            "/hook/UPPER",
            &[("Authorization", &auth)],
            "{}"
        )
        .await
        .0,
        404
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_server_stops_when_asked() {
    let core = Arc::new(Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(serve(listener, core, ServeOptions::default(), async {
        let _ = rx.await;
    }));
    tx.send(()).unwrap();
    assert!(
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
async fn the_page_is_static_self_contained_and_locked_down() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    for path in ["/", "/ui"] {
        let (status, head, body) = call(h.addr, "GET", path, &[], "").await;
        assert_eq!(status, 200, "{path}");
        let lower = head.to_ascii_lowercase();
        assert!(
            lower.contains("content-security-policy: default-src 'none'"),
            "{head}"
        );
        assert!(lower.contains("x-frame-options: deny"));
        assert!(body.contains("<title>ClawDot</title>"));
        // No external assets (REQ-CFG-001).
        // (The W3C namespace names that SVG elements are created with are identifiers, not requests.)
        // (Addresses are also named as plain text, such as the example service addresses in Settings; what matters is
        // that the page loads nothing: no script, style, image or font from anywhere, and no CSS url().)
        let no_namespaces = body.replace("http://www.w3.org/", "");
        for load in [
            "src=\"http",
            "src='http",
            "href=\"http://",
            "url(http",
            "url(\"http",
            "@import",
        ] {
            assert!(
                !no_namespaces.contains(load),
                "external reference in page: {load}"
            );
        }
        // The tokens are inlined: light by default, dark only by explicit choice or match-system, reduced motion handled.
        assert!(!body.contains("/*@theme*/"), "theme marker left in page");
        let squeezed = body.replace(": ", ":");
        assert!(
            squeezed.contains("--color-text-primary:")
                && squeezed.contains("[data-theme=dark]")
                && squeezed.contains("prefers-color-scheme:dark")
        );
        assert!(squeezed.contains("prefers-reduced-motion:reduce"));
        assert!(!body.contains("url("), "the page loads nothing by URL");
        // Agent text must only be rendered as text. The bundled Preact runtime mentions `innerHTML`
        // (it supports a raw-HTML prop), so this is enforced on the portal sources instead
        // (portal/test/unit/rules.test.ts forbids every HTML injection API).
        assert!(
            !body.contains("dangerouslySetInnerHTML:"),
            "the page must not pass raw HTML"
        );
    }
}

// The table is the one list of routes (architecture review C4), here for everything but the portal's `/api`.
#[test]
fn the_server_table_is_well_formed_and_its_classes_are_the_ones_expected() {
    crate::routes::assert_well_formed(ROUTES);
    let by = |a: Auth| -> Vec<String> {
        ROUTES
            .iter()
            .filter(|r| r.auth == a)
            .map(|r| format!("{} {}", r.method, r.path))
            .collect()
    };
    assert_eq!(by(Page), ["GET /", "HEAD /", "GET /ui", "HEAD /ui"]);
    assert_eq!(by(Public), ["GET /healthz"]);
    assert_eq!(
        by(EndpointToken),
        [
            "POST /hook/{endpoint}",
            "GET /bridge/{endpoint}/replies",
            "POST /bridge/{endpoint}/replies/{id}/ack"
        ]
    );
    assert_eq!(by(EndpointOrAdmin), ["POST /api/announce"]);
    assert_eq!(by(AnyEndpointToken), ["POST /mcp"]);
    // No class the portal's own table owns.
    assert!(by(Auth::Open).is_empty() && by(Auth::SignedIn).is_empty());
    // The two tables share no route, so the order in which they are tried cannot matter.
    for (m, p) in crate::ui::api_routes() {
        assert!(
            !ROUTES.iter().any(|r| r.method == m && r.path == p),
            "{m} {p} is in both tables"
        );
    }
}

/// Send the head of a request that promises a body, never send the body, and return the status line if the
/// server answers before its body timeout (a server that read the body first would wait the five seconds out).
async fn answered_without_the_body(
    addr: SocketAddr,
    method: &str,
    path: &str,
    header: Option<(&str, &str)>,
) -> Option<u16> {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 64\r\n"
    );
    if let Some((k, v)) = header {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = [0_u8; 64];
    let n = timeout(Duration::from_millis(1500), s.read(&mut buf))
        .await
        .ok()?
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
}

// REQ-SEC-004, REQ-INB-002: every route that takes a token refuses a missing or wrong one before reading a body.
#[tokio::test]
async fn every_token_route_refuses_a_missing_or_wrong_token_before_reading_the_body() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    // Sign the portal in, so a request without a session is refused as 401 and not as "set up first".
    let _ = admin(&h);
    let guarded: Vec<_> = ROUTES
        .iter()
        .filter(|r| {
            matches!(
                r.auth,
                Auth::EndpointToken | Auth::AnyEndpointToken | Auth::EndpointOrAdmin
            )
        })
        .collect();
    assert!(guarded.len() >= 5, "only {} token routes", guarded.len());
    for route in guarded {
        // The endpoint in the path exists, so the refusal is about the token, not about the name.
        let path = crate::routes::sample_path(&route.path.replace("{endpoint}", "claude-code"));
        let wrong = bearer("cdt_not-the-token");
        for header in [None, Some(("Authorization", wrong.as_str()))] {
            let s = answered_without_the_body(h.addr, route.method, &path, header).await;
            assert!(
                matches!(s, Some(401 | 429)),
                "{} {path} with {header:?}: {s:?}",
                route.method
            );
        }
    }
}

// The ways the routes differ today. These are pinned, not endorsed: a change here is a behaviour change.
#[tokio::test]
async fn the_known_differences_between_routes_are_pinned() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let _ = admin(&h);
    let big = ("Content-Length", "10485760");
    // 1. Body routes check the declared size before the credentials, so an oversized request is 413 even with
    //    no token; the stream and the ack take no body and never answer 413.
    for path in ["/hook/claude-code", "/api/announce", "/mcp"] {
        let mut s = TcpStream::connect(h.addr).await.unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{}: {}\r\n\r\n",
            big.0, big.1
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
            .await
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 413"),
            "{path}"
        );
    }
    // 2. The ack checks that the id is a number before it checks the token: a bad id is 400 with a wrong token
    //    but 401 with none.
    let wrong = bearer("cdt_wrong");
    let bad_id = "/bridge/claude-code/replies/x/ack";
    let (s, ..) = call(h.addr, "POST", bad_id, &[("Authorization", &wrong)], "").await;
    assert_eq!(s, 400);
    assert_eq!(call(h.addr, "POST", bad_id, &[], "").await.0, 401);
    // 3. A wrong method is 405 with no credential on the server's own routes, while the portal's routes ask
    //    for the sign-in first (401) and its open routes answer 404.
    assert_eq!(
        call(h.addr, "GET", "/hook/claude-code", &[], "").await.0,
        405
    );
    assert_eq!(call(h.addr, "GET", "/api/announce", &[], "").await.0, 405);
    assert_eq!(call(h.addr, "GET", "/mcp", &[], "").await.0, 405);
    assert_eq!(call(h.addr, "HEAD", "/healthz", &[], "").await.0, 405);
    assert_eq!(call(h.addr, "GET", "/api/overview", &[], "").await.0, 401);
    assert_eq!(call(h.addr, "PUT", "/api/overview", &[], "").await.0, 401);
    assert_eq!(call(h.addr, "GET", "/api/login", &[], "").await.0, 404);
    // 4. The 405 for the page carries the portal's cache headers; the 405 elsewhere does not.
    let (s, head, _) = call(h.addr, "POST", "/", &[], "").await;
    assert_eq!(s, 405);
    assert!(head.to_lowercase().contains("cache-control: no-store"));
    let (s, head, _) = call(h.addr, "POST", "/healthz", &[], "").await;
    assert_eq!(s, 405);
    assert!(!head.to_lowercase().contains("cache-control"));
    // 5. An endpoint name that is not valid is not a route (404); a valid one that does not exist is a 401.
    let hdr = [("Authorization", wrong.as_str())];
    assert_eq!(call(h.addr, "POST", "/hook/UPPER", &hdr, "{}").await.0, 404);
    assert_eq!(
        call(h.addr, "GET", "/bridge/UPPER/replies", &hdr, "")
            .await
            .0,
        404
    );
    assert_eq!(
        call(h.addr, "POST", "/hook/nobody", &hdr, "{}").await.0,
        401
    );
}

// REQ-CFG-005, threat T5: in open mode the only credential is the caller's address, which a web page in a browser on
// the same network borrows. A "simple" cross-site POST needs no preflight and the body is parsed whatever its type, so
// a state-changing request that names a different origin than the one it was sent to must be refused.
#[tokio::test]
async fn a_web_page_on_another_site_cannot_drive_the_portal_in_open_mode() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let evil = [("Origin", "http://evil.example")];
    // Before setup: a page must not be able to choose the mode for the owner.
    let (s, _, _) = call(h.addr, "POST", "/api/setup", &evil, r#"{"mode":"open"}"#).await;
    assert_eq!(s, 403);
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["mode"], "setup", "nothing was set up");
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );

    let make = r#"{"name":"csrf","adapter":"generic"}"#;
    for origin in [
        "http://evil.example",
        "null",
        "http://127.0.0.1.evil.example",
        "http://127.0.0.1:1",
    ] {
        let hdrs = [("Origin", origin)];
        let (s, _, _) = call(h.addr, "POST", "/api/endpoints", &hdrs, make).await;
        assert_eq!(s, 403, "{origin}");
    }
    assert!(
        h.core.endpoints().iter().all(|e| e.name != "csrf"),
        "no cross-site request changed anything"
    );
    // The announce route takes the administrator path in open mode, so it is covered too.
    let say = r#"{"text":"hello","endpoint":"claude-code"}"#;
    assert_eq!(
        call(h.addr, "POST", "/api/announce", &evil, say).await.0,
        403
    );
    // A browser labels a request made by a page on another site, whatever its method (an image or a script tag is
    // a GET with no Origin). Such a request is refused before it can have any effect.
    for path in [
        "/api/overview",
        "/api/models?url=https://evil.example&key_env=PATH",
    ] {
        let labelled = [("Sec-Fetch-Site", "cross-site")];
        assert_eq!(
            call(h.addr, "GET", path, &labelled, "").await.0,
            403,
            "{path}"
        );
    }
    for site in ["same-origin", "none"] {
        let labelled = [("Sec-Fetch-Site", site)];
        let (s, _, _) = call(h.addr, "GET", "/api/overview", &labelled, "").await;
        assert_eq!(s, 200, "{site}");
    }
    // An older browser sends no label. The model list then still takes nothing from the address: with no saved
    // provider it asks nobody, whatever address and variable the request names.
    let (s, _, body) = call(
        h.addr,
        "GET",
        "/api/models?url=https://evil.example&key_env=PATH",
        &[],
        "",
    )
    .await;
    assert_eq!(s, 409, "{body}");
    // Reading without that label is left to the browser's same-origin rule, and the portal itself (origin = host)
    // works, as does a script that sends no Origin at all.
    assert_eq!(call(h.addr, "GET", "/api/overview", &evil, "").await.0, 200);
    let same = [("Origin", "http://127.0.0.1")];
    assert_eq!(
        call(h.addr, "POST", "/api/endpoints", &same, make).await.0,
        201
    );
    let (s, _, _) = call(h.addr, "POST", "/api/announce", &[], say).await;
    assert_eq!(s, 202);
}

// REQ-CFG-005, threat T5: DNS rebinding. The attacker's own name is re-resolved to the device, so the page, the
// device and the Origin all agree; only the Host header still names the attacker. In open mode the address is the
// credential, so the portal answers only to names that are the device's.
#[tokio::test]
async fn a_rebound_name_cannot_reach_the_portal_but_the_devices_own_names_can() {
    // The machine name is injected: the request path answers for the kept value and reads no file or variable.
    let core = Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    )
    .with_machine_name(Some("Speaker.Example.Lan"));
    let h = start_core(Arc::new(core), ServeOptions::default()).await;
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );
    let port = h.addr.port();
    let ours = vec![
        format!("speaker.example.lan:{port}"),
        format!("speaker:{port}"),
        format!("speaker.local:{port}"),
        format!("127.0.0.1:{port}"),
        format!("192.168.1.20:{port}"),
        format!("[::1]:{port}"),
        format!("localhost:{port}"),
        "10.0.0.7".to_owned(),
    ];
    for host in &ours {
        for (method, path) in [("GET", "/"), ("GET", "/api/overview"), ("GET", "/api/auth")] {
            let (s, _, _) = call_for(h.addr, host, method, path, &[], "").await;
            assert_eq!(s, 200, "{host} {method} {path}");
        }
    }
    let make = r#"{"name":"rebind","adapter":"generic"}"#;
    for host in [
        format!("evil.example:{port}"),
        "evil.example".to_owned(),
        format!("localhost.evil.example:{port}"),
        format!("192.168.1.20.evil.example:{port}"),
        format!("{port}"),
    ] {
        for (method, path, body) in [
            ("GET", "/", ""),
            ("HEAD", "/", ""),
            ("GET", "/ui", ""),
            ("GET", "/api/overview", ""),
            ("GET", "/api/auth", ""),
            ("POST", "/api/endpoints", make),
            ("DELETE", "/api/endpoints/claude-code", ""),
            ("GET", "/api/nothing-here", ""),
            // An announcement with no token takes the administrator path, which open mode leaves to the address.
            (
                "POST",
                "/api/announce",
                r#"{"text":"hi","endpoint":"claude-code"}"#,
            ),
        ] {
            let (s, _, text) = call_for(h.addr, &host, method, path, &[], body).await;
            assert_eq!(s, 403, "{host} {method} {path}");
            if method != "HEAD" {
                assert!(text.contains("public address"), "{text}");
            }
        }
    }
    assert!(
        h.core.endpoints().iter().all(|e| e.name != "rebind"),
        "nothing was changed"
    );
}

// A core answers for its own kept name only: a core built with no name (or another one) refuses "speaker".
#[tokio::test]
async fn the_kept_machine_name_is_per_core() {
    let unnamed = start(Limits::default(), ServeOptions::default()).await;
    let other = Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    )
    .with_machine_name(Some("other"));
    let other = start_core(Arc::new(other), ServeOptions::default()).await;
    for h in [&unnamed, &other] {
        assert_eq!(post(h, "/api/setup", "", r#"{"mode":"open"}"#).await.0, 200);
        let host = format!("speaker:{}", h.addr.port());
        assert_eq!(
            call_for(h.addr, &host, "GET", "/api/auth", &[], "").await.0,
            403
        );
    }
    let host = format!("other:{}", other.addr.port());
    assert_eq!(
        call_for(other.addr, &host, "GET", "/api/auth", &[], "")
            .await
            .0,
        200
    );
}

// Routes that carry their own token cannot be driven by a rebound page, so the Host check leaves them alone.
#[tokio::test]
async fn token_routes_and_the_health_check_work_under_any_host_name() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );
    let auth = bearer(&h.token);
    let hdr = [("Authorization", auth.as_str())];
    let any = "agent.example.net:3335";
    assert_eq!(
        call_for(h.addr, any, "GET", "/healthz", &[], "").await.0,
        200
    );
    assert_eq!(
        call_for(h.addr, any, "POST", "/hook/claude-code", &hdr, STOP)
            .await
            .0,
        202
    );
    assert_eq!(
        call_for(
            h.addr,
            any,
            "POST",
            "/api/announce",
            &hdr,
            r#"{"text":"hello"}"#
        )
        .await
        .0,
        202,
        "an announcement with a token"
    );
    assert_eq!(
        call_for(
            h.addr,
            any,
            "POST",
            "/bridge/claude-code/replies/999/ack",
            &hdr,
            ""
        )
        .await
        .0,
        404,
        "the bridge answers (no such reply); it is not refused for its host"
    );
    assert_eq!(
        call_for(h.addr, any, "POST", "/mcp", &[], "{}").await.0,
        401,
        "the MCP server asks for its token"
    );
    let mut stream = TcpStream::connect(h.addr).await.unwrap();
    let req = format!(
        "GET /bridge/claude-code/replies HTTP/1.1\r\nHost: {any}\r\nAuthorization: Bearer {}\r\n\r\n",
        h.token
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    read_until(&mut stream, &mut buf, "retry:").await;
}
