use super::*;

/// REQ-SEC-011: the host holds the action, asks in its own words, and runs the stored action only after
/// a later turn in which the user said yes.
#[tokio::test(flavor = "current_thread")]
async fn a_risky_call_is_held_and_the_host_asks_then_runs_it_after_a_later_yes() {
    let b = Script::new(vec![
        calls(&[("c1", "send_money", r#"{"amount":5,"to":"sam"}"#)]),
        text("Sent."),
    ]);
    let (mut h, mut c, mut u) = fresh();
    let r1 = turn(&b, &mut h, &mut c, &mut u, "send five dollars to sam").await;
    assert!(h.ran.is_empty(), "nothing ran before the user answered");
    assert!(r1.speech.ends_with("Yes or no?") && r1.speech.contains("send money"));
    assert!(
        r1.speech.contains("amount: 5") && r1.speech.contains("to: sam"),
        "{}",
        r1.speech
    );
    assert_eq!(r1.model_requests, 1, "the host asked; the model did not");
    assert!(r1.pending, "the turn says a held action awaits an answer");
    assert_eq!(r1.held, None, "nothing was answered yet");
    assert_eq!(c.pending().map(PendingAction::tool), Some("send_money"));
    let r2 = turn(&b, &mut h, &mut c, &mut u, "Yes, please.").await;
    assert_eq!(r2.speech, "Sent.");
    assert_eq!(r2.held, Some(HeldOutcome::Ran));
    assert!(!r2.pending, "the answered action is no longer waiting");
    assert_eq!(
        h.ran,
        vec![("send_money".to_owned(), json!({"amount":5,"to":"sam"}))]
    );
    assert!(c.pending().is_none());
    // The answer reached the model as a tool result for the stored call.
    let last = b.requests()[1].messages.last().unwrap().clone();
    assert_eq!(
        (last.role, last.content.as_str()),
        (Role::Tool, "send_money ok")
    );
}

/// REQ-SEC-011: each released action gets its own tool-call id, nine ASCII letters and digits, and its result
/// carries the same id; an id the model made earlier in the history is never reused.
#[tokio::test(flavor = "current_thread")]
async fn released_actions_get_unique_nine_character_call_ids() {
    let (mut h, mut c, mut u) = fresh();
    // The model's own call already uses the id the counter would give first.
    let b = Script::new(vec![
        calls(&[("host00000", "set_timer", r#"{"seconds":1}"#)]),
        text("ok"),
    ]);
    turn(&b, &mut h, &mut c, &mut u, "timer").await;
    for _ in 0..2 {
        let ask = Script::new(vec![calls(&[("c1", "send_money", r#"{"amount":5}"#)])]);
        turn(&ask, &mut h, &mut c, &mut u, "pay").await;
        let done = Script::new(vec![text("Sent.")]);
        turn(&done, &mut h, &mut c, &mut u, "yes").await;
    }
    let ids: Vec<String> = c
        .history()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.clone())
        .filter(|id| id.starts_with("host") && id != "host00000")
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert_ne!(ids[0], ids[1]);
    for id in &ids {
        assert!(
            id.len() == 9 && id.bytes().all(|b| b.is_ascii_alphanumeric()),
            "bad id {id}"
        );
        assert_ne!(id, "host00000", "collided with the model's id");
    }
    let released: Vec<_> = c
        .history()
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .filter(|t| t.name == "send_money" && t.id.starts_with("host"))
        .map(|t| t.id.clone())
        .collect();
    assert_eq!(released, ids, "each result carries its call's id");
}

/// REQ-SEC-011: the user says yes, but the tool is no longer offered: nothing runs, the audit log says why,
/// and the model is told in the same words.
#[tokio::test(flavor = "current_thread")]
async fn a_yes_for_a_tool_that_is_no_longer_offered_runs_nothing() {
    let (log, mut c) = audited(ActionPolicy::default());
    let mut h = Host {
        confirm: vec!["gizmo"],
        extra: vec!["gizmo"],
        ..Host::default()
    };
    let mut u = UsageTracker::new(UsageLimits::default());
    let ask = Script::new(vec![calls(&[("c1", "gizmo", "{}")])]);
    turn(&ask, &mut h, &mut c, &mut u, "use the gizmo").await;
    assert_eq!(c.pending().map(PendingAction::tool), Some("gizmo"));
    h.extra.clear();
    let done = Script::new(vec![text("It is gone.")]);
    let r = turn(&done, &mut h, &mut c, &mut u, "yes").await;
    assert!(h.ran.is_empty(), "nothing ran: {:?}", h.ran);
    assert_eq!(r.tool_calls, 0, "no tool ran, so no call is counted");
    assert_eq!(r.held, Some(HeldOutcome::Gone));
    let seen = done.requests()[0].messages.last().unwrap().clone();
    assert_eq!(
        (seen.role, seen.content.as_str()),
        (Role::Tool, "Error: that tool is no longer available.")
    );
    let r = rows(&log);
    let last = r.last().unwrap();
    assert_eq!(
        (last.decision.as_str(), last.note.as_deref()),
        ("blocked:invalid", Some("tool no longer offered"))
    );
}

/// REQ-SEC-011, REQ-SEC-014: the spoken read-back hides secret-looking values and shortens long ones,
/// while the stored action keeps the exact arguments and runs them after a yes.
#[tokio::test(flavor = "current_thread")]
async fn the_read_back_hides_secrets_but_the_stored_action_is_exact() {
    let args = json!({
        "to": "sam",
        "note": "use Bearer abcdefghijklmnop1234",
        "token": "ghp_0123456789abcdefghij",
        "memo": "a very long memo that goes on and on and on and on and on and on",
    });
    let b = Script::new(vec![
        calls(&[("c1", "send_money", &args.to_string())]),
        text("Sent."),
    ]);
    let (mut h, mut c, mut u) = fresh();
    let r1 = turn(&b, &mut h, &mut c, &mut u, "send sam some money").await;
    for leak in ["abcdefghijklmnop1234", "ghp_0123"] {
        assert!(!r1.speech.contains(leak), "{leak} spoken: {}", r1.speech);
    }
    // A secret is said to be hidden and the call is still held; a long value is read in full.
    assert!(
        r1.speech.contains("token: a hidden key")
            && r1.speech.contains("hidden key that I will not read out")
            && r1.speech.contains("to: sam")
            && r1
                .speech
                .contains("memo: a very long memo that goes on and on and on and on and on and on"),
        "{}",
        r1.speech
    );
    assert!(!r1.speech.contains("a long value"));
    assert!(!c.pending().unwrap().description().contains("abcdefghij"));
    turn(&b, &mut h, &mut c, &mut u, "yes").await;
    assert_eq!(h.ran, vec![("send_money".to_owned(), args)]);
}

/// REQ-SEC-011: a long body within the budget is read in full, and the user hears every argument.
#[tokio::test(flavor = "current_thread")]
async fn a_long_body_within_the_budget_is_read_in_full() {
    let body =
        "Please move the meeting to Thursday at three and bring the signed budget. ".repeat(5);
    let body = body.trim();
    let args = json!({"to": "bob@example.com", "body": body});
    let b = Script::new(vec![
        calls(&[("c1", "send_money", &args.to_string())]),
        text("Sent."),
    ]);
    let (mut h, mut c, mut u) = fresh();
    let r = turn(&b, &mut h, &mut c, &mut u, "tell bob").await;
    assert!(r.speech.contains(body), "{}", r.speech);
    assert!(
        r.speech.contains("to: bob at example dot com"),
        "{}",
        r.speech
    );
    assert!(r.speech.chars().count() < 700 + 40);
    assert!(h.ran.is_empty() && r.pending);
}

/// REQ-SEC-011: a call that cannot be read back within the budget is not held and not run. The model gets an
/// error that says how far over it is, and if the turn ends there the user is told nothing was done, whatever
/// the model says. The same for a proposal from an event turn.
#[tokio::test(flavor = "current_thread")]
async fn a_call_too_long_to_read_back_is_neither_held_nor_run() {
    let long = json!({"to": "bob", "body": "x ".repeat(500)}).to_string();
    let (log, mut c) = audited(ActionPolicy::default());
    let b = Script::new(vec![
        calls(&[("c1", "send_money", &long)]),
        text("Sent it!"),
    ]);
    let (mut h, mut u) = (money_host(), UsageTracker::new(UsageLimits::default()));
    let r = turn(&b, &mut h, &mut c, &mut u, "tell bob everything").await;
    assert!(h.ran.is_empty() && c.pending().is_none() && !r.pending);
    assert!(
        r.speech.starts_with("I did not do that") && !r.speech.contains("Sent it"),
        "{}",
        r.speech
    );
    let seen = b.requests();
    let told = &seen[1].messages.last().unwrap().content;
    assert!(
        told.contains("700") && told.contains("over") && told.contains("nothing was held"),
        "{told}"
    );
    let blocked = rows(&log)
        .into_iter()
        .find(|x| x.tool == "send_money")
        .unwrap();
    assert_eq!(
        (blocked.decision.as_str(), blocked.tier.as_str()),
        ("blocked:invalid", "costly")
    );

    // Shortened on the next try, it is held and asked about, and the speech is the question.
    let b = Script::new(vec![
        calls(&[("c1", "send_money", &long)]),
        calls(&[("c2", "send_money", r#"{"to":"bob","body":"short"}"#)]),
    ]);
    let r = turn(&b, &mut h, &mut c, &mut u, "tell bob").await;
    assert!(
        r.pending && r.speech.ends_with("Yes or no?"),
        "{}",
        r.speech
    );

    // An event turn's proposal is held to the same limit.
    let b = Script::new(vec![calls(&[("e1", "send_money", &long)]), text("ok")]);
    let (mut h, mut c) = (money_host(), conv());
    let r = event_turn("alerts", &b, inputs(&mut h, &mut c, &mut u, 2000), "do it").await;
    assert!(
        c.pending().is_none() && !r.pending && h.ran.is_empty(),
        "{r:?}"
    );
}

/// REQ-SEC-011: two calls that differ only inside a long value are read back differently.
#[tokio::test(flavor = "current_thread")]
async fn calls_that_differ_only_inside_a_long_value_sound_different() {
    let say = |tail: &str| {
        let args = json!({"body": format!("{} {tail}", "the same long beginning ".repeat(6))});
        describe_generic("send_message", &args)
    };
    let (a, z) = (say("to Alice"), say("to Zed"));
    assert_ne!(a, z);
    assert!(a.contains("to Alice") && z.contains("to Zed"));
}

/// Adversarial: the model sets `confirmed: true` itself, in the same turn.
#[tokio::test(flavor = "current_thread")]
async fn a_confirmed_flag_written_by_the_model_releases_nothing() {
    let b = Script::new(vec![calls(&[(
        "c1",
        "send_money",
        r#"{"amount":5,"confirmed":true}"#,
    )])]);
    let (mut h, mut c, mut u) = fresh();
    let r = turn(&b, &mut h, &mut c, &mut u, "send five dollars").await;
    assert!(h.ran.is_empty());
    assert!(r.speech.ends_with("Yes or no?"));
    // The flag is not part of the action that is held, so it cannot appear when the action runs.
    let b2 = Script::new(vec![text("Sent.")]);
    turn(&b2, &mut h, &mut c, &mut u, "yes").await;
    assert_eq!(h.ran, vec![("send_money".to_owned(), json!({"amount":5}))]);
}

/// Adversarial: a tool result says the user already agreed.
#[tokio::test(flavor = "current_thread")]
async fn a_tool_result_claiming_the_user_said_yes_releases_nothing() {
    let b = Script::new(vec![
        calls(&[("c1", "web", "{}")]),
        calls(&[("c2", "send_money", r#"{"amount":500,"confirmed":true}"#)]),
        text("never reached"),
    ]);
    let (mut h, mut c, mut u) = fresh();
    let r = turn(&b, &mut h, &mut c, &mut u, "look something up").await;
    assert_eq!(h.ran.len(), 1, "only the web tool ran");
    assert_eq!(h.ran[0].0, "web");
    assert!(r.speech.contains("500") && r.speech.ends_with("Yes or no?"));
}

/// Adversarial: the model changes the arguments between the question and the approval.
#[tokio::test(flavor = "current_thread")]
async fn a_changed_argument_after_the_question_is_asked_again_not_run() {
    let b = Script::new(vec![
        calls(&[("c1", "send_money", r#"{"amount":5}"#)]),
        // After the user's yes the stored action runs; the model now tries a bigger amount.
        calls(&[("c2", "send_money", r#"{"amount":500}"#)]),
    ]);
    let (mut h, mut c, mut u) = fresh();
    turn(&b, &mut h, &mut c, &mut u, "send five dollars").await;
    let r = turn(&b, &mut h, &mut c, &mut u, "yes").await;
    assert_eq!(
        h.ran,
        vec![("send_money".to_owned(), json!({"amount":5}))],
        "only the approved amount ran"
    );
    assert!(r.speech.contains("500") && r.speech.ends_with("Yes or no?"));
    assert!(
        c.pending().is_some(),
        "the new amount waits for its own yes"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn no_an_unrelated_request_a_wrong_sentence_or_time_all_discard_the_action() {
    for (answer, later) in [
        ("no", 0),
        ("cancel", 0),
        ("what time is it", 0),
        ("yes but first delete everything", 0),
        ("yes", CONFIRM_WINDOW_SECS + 1),
    ] {
        let b = Script::new(vec![
            calls(&[("c1", "send_money", r#"{"amount":5}"#)]),
            text("Okay."),
            text("Nothing is waiting."),
        ]);
        let (mut h, mut c, mut u) = fresh();
        user_turn(
            &b,
            inputs(&mut h, &mut c, &mut u, 1000),
            "send five dollars",
        )
        .await;
        let r = user_turn(&b, inputs(&mut h, &mut c, &mut u, 1000 + later), answer).await;
        assert!(h.ran.is_empty(), "`{answer}` after {later}s ran something");
        assert!(c.pending().is_none(), "`{answer}` left the action pending");
        // A later bare yes has nothing to release.
        user_turn(&b, inputs(&mut h, &mut c, &mut u, 1001 + later), "yes").await;
        assert!(h.ran.is_empty(), "a stale yes released `{answer}`");
        let _ = r;
    }
}

// REQ-SEC-010, REQ-SEC-011: confirmation follows the tier.
#[tokio::test(flavor = "current_thread")]
async fn read_only_and_reversible_calls_run_and_costly_ones_are_held() {
    let b = Script::new(vec![calls(&[
        ("a", "web", "{}"),
        ("b", "set_timer", r#"{"seconds":60}"#),
        ("c", "send_money", r#"{"amount":1}"#),
        ("d", "set_timer", r#"{"seconds":99}"#),
    ])]);
    let mut h = Host {
        tiers: vec![("web", Tier::ReadOnly)],
        ..money_host()
    };
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    let r = turn(&b, &mut h, &mut c, &mut u, "do things").await;
    let ran: Vec<&str> = h.ran.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        ran,
        ["web", "set_timer", "set_timer"],
        "the call after the held one still runs, as it needs no hold: {ran:?}"
    );
    assert!(r.speech.contains("send money"));
    let tool_msgs: Vec<String> = c
        .history()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.clone())
        .collect();
    assert!(tool_msgs[2].starts_with("Not run"), "{tool_msgs:?}");
    assert_eq!(tool_msgs[3], "set_timer ok");
}

#[tokio::test(flavor = "current_thread")]
async fn destructive_tools_are_not_offered_unless_the_administrator_enables_them() {
    let wipe = |allow| {
        let b = Script::new(vec![
            calls(&[("c", "wipe", "{}")]),
            text("I cannot do that."),
        ]);
        let mut h = Host {
            tiers: vec![("wipe", Tier::Destructive)],
            ..Host::default()
        };
        let cfg = TurnConfig {
            allow_destructive: allow,
            ..cfg()
        };
        async move {
            let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
            // `wipe` is not one of the host's specs; add it.
            h.extra.push("wipe");
            let r = user_turn(
                &b,
                inputs_cfg(&mut h, &mut c, &cfg, &mut u, 1000),
                "wipe it",
            )
            .await;
            let offered = b.requests()[0].tools.iter().any(|t| t.name == "wipe");
            (r, h.ran, offered, c.pending().is_some())
        }
    };
    let (r, ran, offered, pending) = wipe(false).await;
    assert!(!offered && ran.is_empty() && !pending);
    assert_eq!(r.speech, "I cannot do that.");
    let (r, ran, offered, pending) = wipe(true).await;
    assert!(
        offered && ran.is_empty() && pending,
        "enabled: offered but still held"
    );
    assert!(r.speech.contains("wipe") && r.speech.ends_with("Yes or no?"));
}

// REQ-SEC-012: untrusted text in the turn overrides an administrator's lowered tier.
#[tokio::test(flavor = "current_thread")]
async fn untrusted_text_makes_a_lowered_costly_tool_ask_for_a_yes_again() {
    let host = || Host {
        lowered: vec!["send_money"],
        tiers: vec![("web", Tier::ReadOnly)],
        ..Host::default()
    };
    // Without outside text, the administrator's override stands and the call runs.
    let b = Script::new(vec![
        calls(&[("a", "send_money", r#"{"amount":1}"#)]),
        text("Sent."),
    ]);
    let (mut h, mut c, mut u) = (host(), conv(), UsageTracker::new(UsageLimits::default()));
    turn(&b, &mut h, &mut c, &mut u, "send one dollar").await;
    assert_eq!(h.ran.len(), 1);
    assert!(!c.is_tainted());

    // After a web result in the same turn it is held.
    let b = Script::new(vec![
        calls(&[("a", "web", "{}")]),
        calls(&[("b", "send_money", r#"{"amount":500}"#)]),
    ]);
    let (mut h, mut c, mut u) = (host(), conv(), UsageTracker::new(UsageLimits::default()));
    let r = turn(&b, &mut h, &mut c, &mut u, "look something up").await;
    assert_eq!(
        h.ran.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["web"]
    );
    assert!(
        c.is_tainted() && r.speech.contains("from outside") && r.speech.ends_with("Yes or no?")
    );

    // The taint stays for the rest of the conversation, and goes with the history when it expires.
    let b = Script::new(vec![
        calls(&[("c", "send_money", r#"{"amount":2}"#)]),
        text("ok"),
    ]);
    user_turn(&b, inputs(&mut h, &mut c, &mut u, 1100), "no").await;
    let r = user_turn(&b, inputs(&mut h, &mut c, &mut u, 1101), "send two dollars").await;
    assert!(
        r.speech.ends_with("Yes or no?") && h.ran.len() == 1,
        "still held: {r:?}"
    );
    let b = Script::new(vec![text("ok")]);
    user_turn(&b, inputs(&mut h, &mut c, &mut u, 1101 + 601), "hello").await;
    assert!(!c.is_tainted());
}

/// REQ-SEC-012: a tool the administrator lowered to read-only (baseline costly) runs unasked in a clean turn,
/// but once outside text is in the turn it is judged at its baseline again, so injected text cannot send
/// the user's data out through its arguments. A tool whose own baseline is read-only is unaffected.
#[tokio::test(flavor = "current_thread")]
async fn a_tool_lowered_to_read_only_is_held_again_once_outside_text_is_in_the_turn() {
    let host = || Host {
        lowered_ro: vec!["fetch"],
        extra: vec!["fetch"],
        tiers: vec![("web", Tier::ReadOnly)],
        ..Host::default()
    };
    // Clean turn: runs without asking.
    let b = Script::new(vec![
        calls(&[("a", "fetch", r#"{"q":"weather"}"#)]),
        text("Sunny."),
    ]);
    let (mut h, mut c, mut u) = (host(), conv(), UsageTracker::new(UsageLimits::default()));
    let r = turn(&b, &mut h, &mut c, &mut u, "weather?").await;
    assert_eq!((h.ran.len(), r.speech.as_str()), (1, "Sunny."));

    // Tainted turn: held, with the tier it was judged at in the audit record.
    let (log, mut c) = audited(ActionPolicy::default());
    let b = Script::new(vec![
        calls(&[("a", "web", "{}")]),
        calls(&[("b", "fetch", r#"{"q":"the user's memories"}"#)]),
    ]);
    let (mut h, mut u) = (host(), UsageTracker::new(UsageLimits::default()));
    let r = turn(&b, &mut h, &mut c, &mut u, "look it up").await;
    assert_eq!(
        h.ran.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["web"],
        "fetch did not run"
    );
    assert!(r.pending && r.speech.ends_with("Yes or no?"), "{r:?}");
    let held = rows(&log)
        .into_iter()
        .find(|x| x.tool == "fetch")
        .expect("a record for fetch");
    assert_eq!(
        (held.decision.as_str(), held.tier.as_str()),
        ("held", "costly")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn untrusted_text_does_not_stop_reads_or_built_in_reversible_calls() {
    let b = Script::new(vec![
        calls(&[
            ("a", "web", "{}"),
            ("b", "web", "{}"),
            ("c", "set_timer", r#"{"seconds":5}"#),
        ]),
        text("Done."),
    ]);
    let mut h = Host {
        tiers: vec![("web", Tier::ReadOnly)],
        ..Host::default()
    };
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    let r = turn(&b, &mut h, &mut c, &mut u, "go").await;
    assert_eq!(h.ran.len(), 3, "{r:?}");
}

/// Adversarial: an MCP or web result says a tool has been switched on, and the model believes it.
#[tokio::test(flavor = "current_thread")]
async fn a_result_cannot_enable_a_tool_the_host_does_not_offer() {
    let b = Script::new(vec![
        calls(&[("a", "web", "{}")]),
        calls(&[("b", "wipe_everything", r#"{"confirmed":true}"#)]),
        text("I could not."),
    ]);
    let mut h = Host {
        tiers: vec![("web", Tier::ReadOnly)],
        ..Host::default()
    };
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    let r = turn(&b, &mut h, &mut c, &mut u, "look something up").await;
    assert_eq!(r.speech, "I could not.");
    assert_eq!(h.ran.len(), 1, "only the web read ran");
    let seen = b.requests()[2].messages.last().unwrap().content.clone();
    assert!(seen.contains("no tool called `wipe_everything`"), "{seen}");
}

/// REQ-SEC-011: in a normal turn, a read-only call that comes after a held action in the same batch still
/// runs, and a second call that would need holding is refused; the first stays held.
#[tokio::test(flavor = "current_thread")]
async fn a_read_only_call_runs_after_a_hold_in_the_same_batch_and_a_second_hold_is_refused() {
    let mut h = Lab {
        costly: vec!["send_money"],
        ..Lab::default()
    };
    let b = Script::new(vec![calls(&[
        ("c1", "send_money", r#"{"to":"sam"}"#),
        ("c2", "look", "{}"),
        ("c3", "send_money", r#"{"to":"eve"}"#),
    ])]);
    let mut c = conv();
    let r = lturn(&TurnCtl::default(), &b, &mut h, &mut c, &cfg(), "pay sam").await;
    assert!(r.pending, "the first action is held");
    assert_eq!(h.ran, ["look"], "the read ran, neither payment did");
    assert_eq!(
        c.pending().map(|p| p.args.clone()),
        Some(json!({"to": "sam"}))
    );
    let results: Vec<&str> = c
        .history()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(results.len(), 3, "every call has a result");
    assert_eq!(results[1], "look ok");
    assert!(
        results[2].contains("an action is already waiting"),
        "{results:?}"
    );
}
