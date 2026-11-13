use super::*;

#[tokio::test]
async fn endpoints_can_be_created_rotated_and_deleted_and_tokens_show_once() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let a = admin(&h);
    let auth = [("Authorization", a.as_str())];

    let (status, _, body) = call(
        h.addr,
        "POST",
        "/api/endpoints",
        &auth,
        r#"{"name":"ci","adapter":"generic"}"#,
    )
    .await;
    assert_eq!(status, 201);
    let token = serde_json::from_str::<serde_json::Value>(&body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    // The new token works for posting events.
    let tb = bearer(&token);
    let (status, ..) = call(
        h.addr,
        "POST",
        "/hook/ci",
        &[("Authorization", tb.as_str())],
        r#"{"title":"build done","text":"ok"}"#,
    )
    .await;
    assert_eq!(status, 202);
    // The overview shows the event but never the token.
    let (_, _, ov) = call(h.addr, "GET", "/api/overview", &auth, "").await;
    assert!(ov.contains("build done") && !ov.contains(&token));

    let (status, ..) = call(
        h.addr,
        "POST",
        "/api/endpoints",
        &auth,
        r#"{"name":"ci","adapter":"generic"}"#,
    )
    .await;
    assert_eq!(status, 409);
    let (status, ..) = call(
        h.addr,
        "POST",
        "/api/endpoints",
        &auth,
        r#"{"name":"x","adapter":"nope"}"#,
    )
    .await;
    assert_eq!(status, 400);
    let (status, ..) = call(
        h.addr,
        "POST",
        "/api/endpoints",
        &auth,
        r#"{"name":"Bad Name","adapter":"generic"}"#,
    )
    .await;
    assert_eq!(status, 400);
    let (status, ..) = call(h.addr, "POST", "/api/endpoints", &auth, "{nope").await;
    assert_eq!(status, 400);

    let (status, _, body) = call(h.addr, "POST", "/api/endpoints/ci/rotate", &auth, "").await;
    assert_eq!(status, 200);
    assert!(body.contains("cdt_") && !body.contains(&token));
    let (status, ..) = call(
        h.addr,
        "POST",
        "/hook/ci",
        &[("Authorization", tb.as_str())],
        r#"{"title":"x"}"#,
    )
    .await;
    assert_eq!(status, 401, "the old token stops working");

    assert_eq!(
        call(h.addr, "DELETE", "/api/endpoints/ci", &auth, "")
            .await
            .0,
        200
    );
    assert_eq!(
        call(h.addr, "DELETE", "/api/endpoints/ci", &auth, "")
            .await
            .0,
        404
    );
    assert_eq!(call(h.addr, "GET", "/api/nothing", &auth, "").await.0, 404);
}

#[tokio::test]
async fn status_and_test_speak_need_the_administrator_and_are_bounded() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let a = admin(&h);
    let auth = [("Authorization", a.as_str())];
    for (m, p) in [("GET", "/api/status"), ("POST", "/api/speak")] {
        assert_eq!(call(h.addr, m, p, &[], "{}").await.0, 401, "{p}");
    }

    h.core
        .set_ui_status(serde_json::json!({"version": "9", "llm": {"configured": false}}));
    let (status, _, body) = call(h.addr, "GET", "/api/status", &auth, "").await;
    assert_eq!(status, 200);
    assert!(
        body.contains(r#""version":"9""#) && body.contains(r#""now""#),
        "{body}"
    );

    let speak = |t: &str| format!(r#"{{"text":"{t}"}}"#);
    assert_eq!(
        call(h.addr, "POST", "/api/speak", &auth, &speak("hello there"))
            .await
            .0,
        202
    );
    assert_eq!(h.core.take_speech().as_deref(), Some("hello there"));
    assert!(h.core.take_speech().is_none());
    // A voice can be tried for one phrase before it is saved (REQ-CFG-081); a nonsense name is refused.
    let with_voice = |v: &str| format!(r#"{{"text":"hi","voice":"{v}"}}"#);
    assert_eq!(
        call(h.addr, "POST", "/api/speak", &auth, &with_voice("nova"))
            .await
            .0,
        202
    );
    assert_eq!(
        h.core.take_speech_as(),
        Some(("hi".to_owned(), Some("nova".to_owned())))
    );
    assert_eq!(
        call(
            h.addr,
            "POST",
            "/api/speak",
            &auth,
            &with_voice(&"v".repeat(129))
        )
        .await
        .0,
        400
    );
    assert!(h.core.take_speech().is_none());

    for bad in [
        speak(""),
        speak("   "),
        speak(&"x".repeat(201)),
        "{nope".to_owned(),
        r#"{"text":"a","extra":1}"#.to_owned(),
    ] {
        assert_eq!(
            call(h.addr, "POST", "/api/speak", &auth, &bad).await.0,
            400,
            "{bad}"
        );
    }
    // Only a few phrases may wait; the rest are refused rather than piling up.
    let mut refused = 0;
    for _ in 0..10 {
        if call(h.addr, "POST", "/api/speak", &auth, &speak("again"))
            .await
            .0
            == 409
        {
            refused += 1;
        }
    }
    assert_eq!(refused, 10 - crate::app::MAX_PENDING_SPEECH);
}

/// REQ-CFG-070: a refused test phrase says why and how to fix it; only a queued one answers `ok: true`.
#[tokio::test]
async fn test_speak_says_why_it_stayed_silent() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let a = admin(&h);
    let auth = [("Authorization", a.as_str())];
    let say = r#"{"text":"hello"}"#;
    let ready = || {
        serde_json::json!({
            "speaker": {"connected": true, "available": true},
            "voice": {"configured": true}, "muted": false,
        })
    };
    let cases = [
        (
            "no_voice",
            503,
            serde_json::json!({"speaker": {"connected": true}, "voice": {"configured": false}}),
        ),
        (
            "no_speaker",
            409,
            serde_json::json!({"speaker": {"connected": false}, "voice": {"configured": true}}),
        ),
        (
            "muted",
            409,
            serde_json::json!({"speaker": {"connected": true, "available": true}, "muted": true}),
        ),
        (
            "busy",
            409,
            serde_json::json!({"speaker": {"connected": true, "available": false}}),
        ),
    ];
    for (reason, code, status) in cases {
        h.core.set_ui_status(status);
        let (s, _, body) = call(h.addr, "POST", "/api/speak", &auth, say).await;
        let v = json_of(&body);
        assert_eq!(s, code, "{reason}: {body}");
        assert_eq!(v["ok"], false, "{reason}");
        assert_eq!(v["reason"], reason, "{body}");
        assert!(
            v["message"].as_str().is_some_and(|m| m.len() > 20),
            "{reason} must explain itself: {body}"
        );
        assert!(
            h.core.take_speech().is_none(),
            "{reason} must queue nothing"
        );
    }
    // A speaker that can speak takes it; so does a status that does not say (not published yet).
    h.core.set_ui_status(ready());
    let (s, _, body) = call(h.addr, "POST", "/api/speak", &auth, say).await;
    assert_eq!(s, 202, "{body}");
    assert_eq!(json_of(&body)["ok"], true);
    h.core.set_ui_status(serde_json::json!({"version": "9"}));
    assert_eq!(call(h.addr, "POST", "/api/speak", &auth, say).await.0, 202);
    // Speech printed to the terminal needs neither a satellite nor a voice.
    h.core.set_ui_status(serde_json::json!({
        "speaker": {"connected": true, "available": true, "name": "console"},
        "voice": {"configured": false},
    }));
    assert_eq!(call(h.addr, "POST", "/api/speak", &auth, say).await.0, 202);
    // The administrator is still required.
    assert_eq!(call(h.addr, "POST", "/api/speak", &[], say).await.0, 401);
}
