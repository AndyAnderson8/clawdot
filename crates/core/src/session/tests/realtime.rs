use super::*;

/// REQ-SEC-011: a model outside the turn loop (the realtime voice) that calls more tools while an action
/// waits for the user does not cancel it. Reads and reversible calls run; a call that would need holding is
/// refused; the words that name the turn are not an answer, even a "yes".
#[tokio::test(flavor = "current_thread")]
async fn proposed_calls_leave_a_held_action_waiting_and_refuse_a_second_hold() {
    let mut h = Host {
        confirm: vec!["send_money"],
        ..Host::default()
    };
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    let first = [call("c1", "send_money", r#"{"to":"sam"}"#)];
    run_proposed_calls(inputs(&mut h, &mut c, &mut u, 1000), "pay sam", &first).await;
    assert_eq!(
        c.pending().map(|p| p.args.clone()),
        Some(json!({"to": "sam"}))
    );

    let more = [
        call("c2", "set_timer", r#"{"seconds":5}"#),
        call("c3", "send_money", r#"{"to":"eve"}"#),
    ];
    // The text passed is "yes": a model's calls are never an answer.
    run_proposed_calls(inputs(&mut h, &mut c, &mut u, 1030), "yes", &more).await;
    assert_eq!(
        c.pending().map(|p| p.args.clone()),
        Some(json!({"to": "sam"})),
        "the first action is still the one waiting"
    );
    let names: Vec<&str> = h.ran.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["set_timer"], "only the other call ran");
    let history = format!("{:?}", c.history());
    assert!(
        history.contains("an action is already waiting for the user's answer"),
        "{history}"
    );

    // The user's own yes, read by the host, runs the first action.
    answer_held_action(inputs(&mut h, &mut c, &mut u, 1040), "yes").await;
    assert_eq!(h.ran.last().unwrap().0, "send_money");
    assert_eq!(h.ran.last().unwrap().1, json!({"to": "sam"}));
    assert!(c.pending().is_none());
}

/// REQ-SEC-011: a realtime batch follows the same rule: a call that needs no hold runs after one that does.
#[tokio::test(flavor = "current_thread")]
async fn proposed_calls_run_a_later_call_after_a_hold_in_the_same_batch() {
    let mut h = Host {
        confirm: vec!["send_money"],
        ..Host::default()
    };
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    let batch = [
        call("c1", "send_money", r#"{"to":"sam"}"#),
        call("c2", "set_timer", r#"{"seconds":5}"#),
        call("c3", "send_money", r#"{"to":"eve"}"#),
    ];
    run_proposed_calls(inputs(&mut h, &mut c, &mut u, 1000), "pay sam", &batch).await;
    let names: Vec<&str> = h.ran.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["set_timer"]);
    assert_eq!(
        c.pending().map(|p| p.args.clone()),
        Some(json!({"to": "sam"}))
    );
}

// REQ-SEC-011: the 120-second window still ends a held action that nobody answered, and then a new call can
// be held.
#[tokio::test(flavor = "current_thread")]
async fn proposed_calls_do_not_keep_a_held_action_past_its_window() {
    let mut h = Host {
        confirm: vec!["send_money"],
        ..Host::default()
    };
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    run_proposed_calls(
        inputs(&mut h, &mut c, &mut u, 1000),
        "pay sam",
        &[call("c1", "send_money", r#"{"to":"sam"}"#)],
    )
    .await;
    let late = 1000 + CONFIRM_WINDOW_SECS + 1;
    run_proposed_calls(
        inputs(&mut h, &mut c, &mut u, late),
        "",
        &[call("c2", "send_money", r#"{"to":"eve"}"#)],
    )
    .await;
    assert_eq!(
        c.pending().map(|p| p.args.clone()),
        Some(json!({"to": "eve"}))
    );
    assert!(h.ran.is_empty());
}
