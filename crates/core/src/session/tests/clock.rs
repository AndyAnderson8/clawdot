use super::*;

// REQ-SEC-011: the 120 s window is a duration.
#[tokio::test(flavor = "current_thread")]
async fn a_held_action_expires_after_120_monotonic_seconds_whatever_the_wall_clock_does() {
    // (wall step between the question and the yes, monotonic seconds between them, is the yes honoured)
    for (step, later, released) in [
        (-3 * HOUR, CONFIRM_WINDOW_SECS + 1, false), // clock back: must NOT stay releasable
        (3 * HOUR, CONFIRM_WINDOW_SECS + 1, false),
        (-3 * HOUR, CONFIRM_WINDOW_SECS, true),
        (-3 * HOUR, 30, true),
        (3 * HOUR, 60, true), // clock forward: must NOT expire early
        (3 * HOUR, CONFIRM_WINDOW_SECS, true),
        (-HOUR, 1, true),
    ] {
        let b = Script::new(vec![
            calls(&[("c1", "send_money", r#"{"amount":5}"#)]),
            text("Sent."),
        ]);
        let (mut h, mut c, _) = fresh();
        stepped(&b, &mut h, &mut c, (5000, 1000), "send five dollars").await;
        assert!(c.pending().is_some());
        let at = (5000 + step + later, 1000 + later);
        let r = stepped(&b, &mut h, &mut c, at, "yes").await;
        assert_eq!(
            !h.ran.is_empty(),
            released,
            "step {step} s, {later} s on the monotonic clock: {r:?}"
        );
        if !released {
            assert_eq!(r.held, Some(HeldOutcome::Expired));
        }
    }
}

// The same for an event turn, which drops a stale proposal at once.
#[tokio::test(flavor = "current_thread")]
async fn an_event_turn_drops_a_stale_proposal_by_monotonic_time_too() {
    let b = Script::new(vec![calls(&[("c1", "send_money", r#"{"amount":5}"#)])]);
    let (mut h, mut c, mut u) = fresh();
    user_turn(
        &b,
        inputs(&mut h, &mut c, &mut u, 1000),
        "send five dollars",
    )
    .await;
    assert!(c.pending().is_some());
    let quiet = Script::new(vec![text("Noted.")]);
    // The wall clock went back three hours, but 100 s passed: still held.
    let back = TurnInputs {
        now: 1000 - 3 * HOUR,
        mono: 1100,
        ..inputs(&mut h, &mut c, &mut u, 0)
    };
    event_turn("hook", &quiet, back, "build done").await;
    assert!(c.pending().is_some(), "100 s is inside the window");
    // The wall clock went forward three hours, and 121 s passed since the proposal: dropped.
    let forward = TurnInputs {
        now: 1000 + 3 * HOUR,
        mono: 1121,
        ..inputs(&mut h, &mut c, &mut u, 0)
    };
    event_turn("hook", &quiet, forward, "build done").await;
    assert!(c.pending().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_clarifying_question_expires_after_120_monotonic_seconds_whatever_the_wall_clock_does() {
    for (step, later, still_open) in [
        (-3 * HOUR, CONFIRM_WINDOW_SECS + 1, false),
        (3 * HOUR, CONFIRM_WINDOW_SECS + 1, false),
        (-3 * HOUR, CONFIRM_WINDOW_SECS, true),
        (3 * HOUR, 60, true),
    ] {
        let mut h = Lab::default();
        let b = Script::new(vec![
            calls(&[("c1", "ask_user", r#"{"question":"Which room?"}"#)]),
            text("Fine."),
        ]);
        let mut c = conv();
        stepped(&b, &mut h, &mut c, (5000, 1000), "turn on the light").await;
        assert!(c.is_asking());
        let at = (5000 + step + later, 1000 + later);
        let r = stepped(&b, &mut h, &mut c, at, "never mind").await;
        // Inside the window "never mind" is answered by the host without the model.
        assert_eq!(
            r.speech == "Okay, never mind.",
            still_open,
            "step {step} s, {later} s: {}",
            r.speech
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_forget_all_confirmation_is_a_duration_on_the_monotonic_clock_too() {
    for (step, later, forgotten) in [
        (-3 * HOUR, CONFIRM_WINDOW_SECS + 1, false),
        (3 * HOUR, CONFIRM_WINDOW_SECS + 1, false),
        (-3 * HOUR, 30, true),
        (3 * HOUR, 60, true),
    ] {
        let (mut c, store) = with_notes(true);
        store.add("my printer is called Pixel", 5).unwrap();
        let mut h = Lab::default();
        let b = Script::new(vec![text("Hm.")]);
        stepped(&b, &mut h, &mut c, (5000, 1000), "forget everything").await;
        let at = (5000 + step + later, 1000 + later);
        stepped(&b, &mut h, &mut c, at, "yes").await;
        assert_eq!(
            store.list().is_empty(),
            forgotten,
            "step {step} s, {later} s on the monotonic clock"
        );
    }
}

// REQ-AST-007
#[tokio::test(flavor = "current_thread")]
async fn conversation_idle_expiry_follows_monotonic_time() {
    for (step, later, kept) in [
        (-3 * HOUR, 601, false), // a clock stepped back must not keep it alive
        (3 * HOUR, 601, false),
        (3 * HOUR, 60, true), // a clock stepped forward must not wipe it
        (-3 * HOUR, 600, true),
    ] {
        let b = Script::new(vec![text("one"), text("two")]);
        let (mut h, mut c, _) = fresh();
        stepped(&b, &mut h, &mut c, (5000, 1000), "a").await;
        let at = (5000 + step + later, 1000 + later);
        stepped(&b, &mut h, &mut c, at, "b").await;
        assert_eq!(
            c.history().len() == 4,
            kept,
            "step {step} s, {later} s on the monotonic clock"
        );
    }
}
