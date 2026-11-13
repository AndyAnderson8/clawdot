use super::*;

// ----- the audit log, caps and dry-run (REQ-SEC-014..016) -----

// REQ-SEC-014
#[tokio::test(flavor = "current_thread")]
async fn every_call_and_the_users_answer_are_recorded() {
    let (log, mut c) = audited(ActionPolicy::default());
    let (mut h, mut u) = (money_host(), UsageTracker::new(UsageLimits::default()));
    let b = Script::new(vec![calls(&[
        ("a", "set_timer", r#"{"seconds":60}"#),
        (
            "b",
            "send_money",
            r#"{"amount":5,"api_key":"sk-live-0123456789abcdef","note":"Bearer abcdefghijklmnop"}"#,
        ),
    ])]);
    let r = turn(&b, &mut h, &mut c, &mut u, "pay sam").await;
    assert!(r.speech.ends_with("Yes or no?"));
    let b2 = Script::new(vec![text("Sent.")]);
    turn(&b2, &mut h, &mut c, &mut u, "yes").await;
    assert_eq!(
        pairs(&log),
        [
            pair("set_timer", "ran"),
            pair("send_money", "held"),
            pair("send_money", "approved"),
        ]
    );
    let all = rows(&log);
    assert_eq!(all[0].trigger, "voice");
    assert_eq!(
        (all[0].tier.as_str(), all[0].status.as_deref()),
        ("reversible", Some("ok"))
    );
    assert_eq!(
        all[2].held_id,
        Some(all[1].id),
        "the approval points at the question"
    );
    assert_eq!(all[2].status.as_deref(), Some("ok"));
    assert_ne!(all[0].turn, all[2].turn, "the yes is a later turn");
    // the real arguments reached the tool; the log never saw the secrets
    assert_eq!(h.ran[1].1["api_key"], "sk-live-0123456789abcdef");
    let text = serde_json::to_string(&all).unwrap();
    assert!(
        !text.contains("sk-live") && !text.contains("abcdefghijklmnop"),
        "{text}"
    );
    assert!(text.contains("[redacted]"));
}

// REQ-SEC-014
#[tokio::test(flavor = "current_thread")]
async fn declined_expired_and_replaced_actions_are_recorded() {
    let (log, mut c) = audited(ActionPolicy::default());
    let (mut h, mut u) = (money_host(), UsageTracker::new(UsageLimits::default()));
    let ask = || calls(&[("c", "send_money", r#"{"amount":5}"#)]);
    // no
    turn(&Script::new(vec![ask()]), &mut h, &mut c, &mut u, "pay").await;
    turn(&Script::new(vec![]), &mut h, &mut c, &mut u, "no").await;
    // unrelated request
    turn(&Script::new(vec![ask()]), &mut h, &mut c, &mut u, "pay").await;
    let noon = Script::new(vec![text("It is noon.")]);
    turn(&noon, &mut h, &mut c, &mut u, "what time is it").await;
    // yes, but too late
    turn(&Script::new(vec![ask()]), &mut h, &mut c, &mut u, "pay").await;
    let none = Script::new(vec![]);
    let late = user_turn(&none, inputs(&mut h, &mut c, &mut u, 1130), "yes").await;
    assert!(late.speech.contains("too long"));
    assert_eq!(late.held, Some(HeldOutcome::Expired));
    // asked, then the conversation goes idle
    turn(&Script::new(vec![ask()]), &mut h, &mut c, &mut u, "pay").await;
    let hi = Script::new(vec![text("Hi.")]);
    user_turn(&hi, inputs(&mut h, &mut c, &mut u, 1830), "hello").await;
    assert!(h.ran.is_empty());
    let d: Vec<String> = rows(&log).into_iter().map(|r| r.decision).collect();
    assert_eq!(
        d,
        [
            "held", "declined", "held", "declined", "held", "expired", "held", "expired"
        ]
    );
    let r = rows(&log);
    assert_eq!(r[1].note.as_deref(), Some("the user said no"));
    assert_eq!(r[3].note.as_deref(), Some("unrelated request"));
    assert_eq!(r[1].held_id, Some(r[0].id));
}

// REQ-SEC-014: a model that misbehaves leaves a trail and gets nothing done.
#[tokio::test(flavor = "current_thread")]
async fn bad_and_forged_calls_are_recorded_as_blocked_or_held() {
    let (log, mut c) = audited(ActionPolicy::default());
    let (mut h, mut u) = (money_host(), UsageTracker::new(UsageLimits::default()));
    let b = Script::new(vec![calls(&[
        ("a", "no_such_tool", "{}"),
        ("b", "set_timer", "{not json"),
        ("c", "send_money", r#"{"amount":5,"confirmed":true}"#),
        ("d", "set_timer", r#"{"seconds":1}"#),
    ])]);
    turn(&b, &mut h, &mut c, &mut u, "go").await;
    let ran: Vec<&str> = h.ran.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(ran, ["set_timer"], "the valid call after the hold ran");
    assert_eq!(
        pairs(&log),
        [
            pair("no_such_tool", "blocked:invalid"),
            pair("set_timer", "blocked:invalid"),
            pair("send_money", "held"),
            pair("set_timer", "ran"),
        ]
    );
    assert_eq!(rows(&log)[2].args, json!({"amount":5}));
}

// REQ-SEC-016
#[tokio::test(flavor = "current_thread")]
async fn dry_run_describes_actions_runs_reads_and_marks_the_log() {
    let (log, mut c) = audited(ActionPolicy {
        dry_run: true,
        ..ActionPolicy::default()
    });
    let mut h = Host {
        tiers: vec![("web", Tier::ReadOnly)],
        ..money_host()
    };
    let mut u = UsageTracker::new(UsageLimits::default());
    let b = Script::new(vec![
        calls(&[
            ("a", "web", "{}"),
            ("b", "set_timer", r#"{"seconds":60}"#),
            ("c", "send_money", r#"{"amount":5}"#),
        ]),
        text("I would set a timer and send money."),
    ]);
    let r = turn(&b, &mut h, &mut c, &mut u, "do things").await;
    assert_eq!(r.speech, "I would set a timer and send money.");
    let names: Vec<&str> = h.ran.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["web"], "only the read ran");
    assert!(c.pending().is_none(), "nothing is asked in dry-run mode");
    assert_eq!(
        pairs(&log),
        [
            pair("web", "ran"),
            pair("set_timer", "blocked:dry-run"),
            pair("send_money", "blocked:dry-run"),
        ]
    );
    assert!(rows(&log)[1].dry_run && !rows(&log)[0].dry_run);
    let tool_msgs: Vec<String> = b.requests()[1]
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.clone())
        .collect();
    assert!(
        tool_msgs[1].starts_with("Dry run: nothing was done")
            && tool_msgs[2].contains("send money")
    );
}

// REQ-SEC-015
#[tokio::test(flavor = "current_thread")]
async fn the_daily_cap_stops_further_actions_and_says_so() {
    let (log, mut c) = audited(ActionPolicy {
        dry_run: false,
        max_actions_per_day: 2,
        max_costly_per_day: 1,
    });
    let mut h = Host {
        tiers: vec![("web", Tier::ReadOnly)],
        ..money_host()
    };
    let mut u = UsageTracker::new(UsageLimits::default());
    let timer = || calls(&[("t", "set_timer", r#"{"seconds":5}"#)]);
    let b = Script::new(vec![timer(), text("ok")]);
    turn(&b, &mut h, &mut c, &mut u, "one").await;
    let b = Script::new(vec![timer(), text("ok")]);
    turn(&b, &mut h, &mut c, &mut u, "two").await;
    let b = Script::new(vec![timer(), text("The daily limit stopped it.")]);
    let r = turn(&b, &mut h, &mut c, &mut u, "three").await;
    assert_eq!(r.speech, "The daily limit stopped it.");
    assert_eq!(h.ran.len(), 2, "the third timer never ran");
    let result = b.requests()[1].messages.last().unwrap().content.clone();
    assert!(
        result.starts_with("Not run: the limit of 2 actions a day"),
        "{result}"
    );
    // reads are not limited
    let b = Script::new(vec![calls(&[("w", "web", "{}")]), text("ok")]);
    turn(&b, &mut h, &mut c, &mut u, "look").await;
    assert_eq!(h.ran.len(), 3);
    // the next day starts clean
    let b = Script::new(vec![timer(), text("ok")]);
    user_turn(
        &b,
        TurnInputs {
            day: 2,
            ..inputs(&mut h, &mut c, &mut u, 1100)
        },
        "again",
    )
    .await;
    assert_eq!(h.ran.len(), 4);
    let d: Vec<String> = rows(&log).into_iter().map(|r| r.decision).collect();
    assert_eq!(d, ["ran", "ran", "blocked:cap", "ran", "ran"]);
}

// REQ-SEC-015: the costly cap applies before the question is asked and again when the yes arrives.
#[tokio::test(flavor = "current_thread")]
async fn the_costly_cap_applies_to_held_actions() {
    let policy = ActionPolicy {
        dry_run: false,
        max_actions_per_day: 10,
        max_costly_per_day: 1,
    };
    let (log, mut c) = audited(policy);
    let (mut h, mut u) = (money_host(), UsageTracker::new(UsageLimits::default()));
    let pay = || calls(&[("p", "send_money", r#"{"amount":5}"#)]);
    turn(&Script::new(vec![pay()]), &mut h, &mut c, &mut u, "pay").await;
    turn(
        &Script::new(vec![text("Sent.")]),
        &mut h,
        &mut c,
        &mut u,
        "yes",
    )
    .await;
    assert_eq!(h.ran.len(), 1);
    let b = Script::new(vec![pay(), text("I can't, the limit is used up.")]);
    let r = turn(&b, &mut h, &mut c, &mut u, "pay again").await;
    assert!(
        c.pending().is_none(),
        "no point asking for something that cannot run"
    );
    assert_eq!(r.speech, "I can't, the limit is used up.");
    assert_eq!(h.ran.len(), 1);
    let d: Vec<String> = rows(&log).into_iter().map(|r| r.decision).collect();
    assert_eq!(d, ["held", "approved", "blocked:cap"]);
    // a cap that fills between the question and the yes still stops it
    let (log2, mut c2) = audited(policy);
    let mut h2 = money_host();
    turn(&Script::new(vec![pay()]), &mut h2, &mut c2, &mut u, "pay").await;
    log2.record(
        1,
        1,
        99,
        "voice",
        None,
        &crate::audit::Act {
            tool: "other",
            tier: Some(Tier::Costly),
            args: &Value::Null,
            decision: decision::RAN,
            held_id: None,
            outcome: None,
            note: None,
        },
    );
    let sorry = Script::new(vec![text("Sorry.")]);
    turn(&sorry, &mut h2, &mut c2, &mut u, "yes").await;
    assert!(h2.ran.is_empty());
    let last = rows(&log2).last().map(|r| r.decision.clone());
    assert_eq!(last.as_deref(), Some("blocked:cap"));
}

// REQ-SEC-014: the turn's model cost rides along when the provider reported one.
#[tokio::test(flavor = "current_thread")]
async fn the_turns_model_cost_is_recorded_when_known() {
    let (log, mut c) = audited(ActionPolicy::default());
    let (mut h, mut u) = (Host::default(), UsageTracker::new(UsageLimits::default()));
    let mut first = calls(&[("a", "set_timer", r#"{"seconds":5}"#)]);
    if let Ok(comp) = &mut first {
        comp.usage = Some(crate::llm::Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            cost_usd: Some(0.0042),
        });
    }
    turn(
        &Script::new(vec![first, text("ok")]),
        &mut h,
        &mut c,
        &mut u,
        "timer",
    )
    .await;
    let r = rows(&log);
    assert!((r[0].turn_cost_usd.unwrap() - 0.0042).abs() < 1e-9);
    // and an estimate from the sizes when the provider reported nothing (REQ-AST-011): never none, so the
    // spend cap and the audit log agree
    let again = Script::new(vec![
        calls(&[("a", "set_timer", r#"{"seconds":5}"#)]),
        text("ok"),
    ]);
    turn(&again, &mut h, &mut c, &mut u, "again").await;
    assert!(rows(&log)[1].turn_cost_usd.unwrap() > 0.0);
}

#[tokio::test(flavor = "current_thread")]
async fn the_full_text_of_a_long_result_is_in_the_audit_record() {
    let (log, mut c) = audited(ActionPolicy::default());
    let mut h = Lab {
        long: 1500,
        ..Lab::default()
    };
    let b = Script::new(vec![
        calls(&[("c1", "report", "{}")]),
        text("The summary."),
        text("ok"),
    ]);
    lturn(&TurnCtl::default(), &b, &mut h, &mut c, &cfg(), "read it").await;
    let rec = rows(&log).into_iter().find(|r| r.tool == "report").unwrap();
    assert_eq!(rec.detail.as_deref().map(str::len), Some(1500));
    assert!(
        rec.result.as_deref().is_some_and(|r| r.len() < 300),
        "the short excerpt stays short"
    );
}
