use super::*;
use crate::llm::{Message, StreamAccumulator, ToolSpec, parse_completion};
use crate::store::testutil::TempDir;
use serde_json::json;

fn capped(cap: f64) -> UsageLimits {
    UsageLimits {
        daily_spend_cap_usd: Some(cap),
        ..UsageLimits::default()
    }
}

fn usage(cost: Option<f64>) -> Usage {
    Usage {
        prompt_tokens: 10,
        completion_tokens: 5,
        cost_usd: cost,
    }
}

fn tokens(prompt_tokens: u32, completion_tokens: u32) -> Usage {
    Usage {
        prompt_tokens,
        completion_tokens,
        cost_usd: None,
    }
}

fn basis_of(t: &UsageTracker) -> Option<&'static str> {
    t.basis().map(Basis::as_str)
}

#[test]
fn added_cost_counts_toward_the_spend_cap_without_counting_a_request() {
    let t = UsageTracker::new(capped(0.10));
    t.add_cost(1, 0.04);
    t.add_cost(1, f64::NAN);
    t.add_cost(1, -5.0);
    assert_eq!(t.requests_today(), 0);
    assert!(t.check(1).is_ok());
    t.add_cost(1, 0.07);
    assert_eq!(t.check(1), Err(LimitReached::Spend));
    assert_eq!(basis_of(&t), Some("priced"), "audio is time at set rates");
    assert_eq!(t.check(2), Ok(()), "a new day starts at zero");
    assert_eq!(basis_of(&t), None, "and with nothing that cost anything");
}

#[test]
fn past_days_are_kept_for_the_chart() {
    let t = UsageTracker::new(UsageLimits::default());
    t.record_request(100);
    t.record_request(100);
    t.record_request(102);
    let (today, days) = t.request_days();
    assert_eq!(today, 102);
    assert_eq!(
        days,
        vec![(100, 2), (102, 1)],
        "a day with no requests is not listed"
    );
    for d in 103..160 {
        t.record_request(d);
    }
    assert!(
        t.request_days().1.len() <= KEEP_DAYS + 1,
        "only recent days are kept"
    );
}

#[test]
fn the_request_cap_stops_requests_and_resets_the_next_day() {
    let t = UsageTracker::new(UsageLimits {
        max_requests_per_day: 2,
        ..UsageLimits::default()
    });
    for _ in 0..2 {
        t.check(100).unwrap();
        t.record_request(100);
    }
    assert_eq!(t.check(100), Err(LimitReached::Requests));
    assert!(t.check(101).is_ok(), "a new local day starts fresh");
    assert_eq!(t.requests_today(), 0);
}

// REQ-AST-011, basis 1: what the provider says it cost is used as it is, whatever prices are set.
#[test]
fn a_cost_the_provider_reports_is_used_as_it_is() {
    let t = UsageTracker::new(UsageLimits {
        llm_usd_per_mtok_in: Some(1000.0),
        ..capped(0.01)
    });
    t.record_sized(1, Some(&usage(Some(0.004))), 9_999, 9_999);
    assert!(t.check(1).is_ok());
    t.record_sized(1, Some(&usage(Some(0.007))), 9_999, 9_999);
    assert_eq!(t.check(1), Err(LimitReached::Spend));
    assert!((t.spend_today() - 0.011).abs() < 1e-9);
    assert_eq!(basis_of(&t), Some("reported"));
}

// Basis 2: tokens the provider counted, times the person's prices. 1000 in at $2 and 500 out at $8 per
// million are 2000 + 4000 millionths of a dollar.
#[test]
fn reported_tokens_are_priced_with_the_prices_the_person_set() {
    let t = UsageTracker::new(UsageLimits {
        llm_usd_per_mtok_in: Some(2.0),
        llm_usd_per_mtok_out: Some(8.0),
        ..capped(0.01)
    });
    t.record_sized(1, Some(&tokens(1000, 500)), 123_456, 654_321);
    assert!(
        (t.spend_today() - 0.006).abs() < 1e-12,
        "{}",
        t.spend_today()
    );
    assert_eq!(basis_of(&t), Some("priced"));
    assert!(t.check(1).is_ok());
    t.record_sized(1, Some(&tokens(1000, 500)), 0, 0);
    assert_eq!(t.check(1), Err(LimitReached::Spend));
}

// A reported cost of 0 (a free model on OpenRouter) is no usable cost, so the tokens are priced.
#[test]
fn a_reported_cost_of_zero_falls_to_the_tokens() {
    let t = UsageTracker::new(UsageLimits {
        llm_usd_per_mtok_in: Some(0.0),
        llm_usd_per_mtok_out: Some(0.0),
        ..UsageLimits::default()
    });
    let u = Usage {
        cost_usd: Some(0.0),
        ..tokens(1000, 500)
    };
    t.record_sized(1, Some(&u), 4000, 2000);
    assert_eq!((t.spend_today(), basis_of(&t)), (0.0, Some("priced")));
}

// Basis 3: nothing reported. 3000 bytes sent are 1000 tokens at the built-in $10 per million, 300 bytes
// received are 100 tokens at $40: 10000 + 4000 millionths, 0.014 a request. A cap of 0.05 lets four
// requests in (0.042 spent after three) and stops the fifth.
#[test]
fn with_nothing_reported_the_size_is_priced_at_the_built_in_prices_and_the_cap_bites() {
    let t = UsageTracker::new(capped(0.05));
    for n in 1..=4 {
        assert!(t.check(1).is_ok(), "request {n} is allowed");
        t.record_sized(1, None, 3000, 300);
    }
    assert!(
        (t.spend_today() - 0.056).abs() < 1e-12,
        "{}",
        t.spend_today()
    );
    assert_eq!(t.check(1), Err(LimitReached::Spend));
    assert_eq!(basis_of(&t), Some("estimated"));
    assert_eq!(t.requests_today(), 4);
}

#[test]
fn a_size_is_rounded_up_to_whole_tokens_and_a_small_request_is_never_free() {
    let (micro, basis) = price_request(&UsageLimits::default(), None, 1, 0);
    assert_eq!(
        (micro, basis),
        (10, Basis::Estimated),
        "one token at $10 per million"
    );
    let cheap = UsageLimits {
        llm_usd_per_mtok_in: Some(0.1),
        ..UsageLimits::default()
    };
    let (micro, _) = price_request(&cheap, Some(&tokens(3, 0)), 0, 0);
    assert_eq!(micro, 1, "0.3 millionths count as one");
}

// Reported tokens are better than a size, but at the built-in prices the basis is still an estimate.
#[test]
fn reported_tokens_at_the_built_in_prices_are_an_estimate() {
    let (micro, basis) = price_request(&UsageLimits::default(), Some(&tokens(1000, 100)), 0, 0);
    assert_eq!((micro, basis), (14_000, Basis::Estimated));
    let one_set = UsageLimits {
        llm_usd_per_mtok_in: Some(1.0),
        ..UsageLimits::default()
    };
    let (micro, basis) = price_request(&one_set, Some(&tokens(1000, 100)), 0, 0);
    assert_eq!(
        (micro, basis),
        (1000 + 4000, Basis::Estimated),
        "one price set, the other built in"
    );
}

#[test]
fn prices_of_zero_mean_a_free_server_with_nothing_estimated() {
    let free = UsageLimits {
        llm_usd_per_mtok_in: Some(0.0),
        llm_usd_per_mtok_out: Some(0.0),
        ..capped(0.01)
    };
    let t = UsageTracker::new(free);
    for _ in 0..50 {
        t.record_sized(1, None, 30_000, 3000);
        t.record_sized(1, Some(&tokens(9000, 900)), 1, 1);
    }
    assert_eq!((t.spend_today(), basis_of(&t)), (0.0, Some("priced")));
    assert!(t.check(1).is_ok());
    assert_eq!(t.requests_today(), 100);
}

#[test]
fn bad_cost_values_are_ignored() {
    let t = UsageTracker::new(UsageLimits::default());
    for c in [f64::NAN, f64::INFINITY, -1.0] {
        let u = Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: Some(c),
        };
        t.record_sized(1, Some(&u), 0, 0);
    }
    assert_eq!(t.spend_today(), 0.0);
    assert_eq!(t.requests_today(), 3);
}

#[test]
fn a_failed_request_counts_but_costs_nothing_and_an_abandoned_one_costs_its_prompt() {
    let t = UsageTracker::new(UsageLimits::default());
    t.record_request(1);
    assert_eq!(
        (t.requests_today(), t.spend_today(), t.basis()),
        (1, 0.0, None)
    );
    let req = ChatRequest {
        model: "m".into(),
        messages: vec![Message::user("x".repeat(3000))],
        tools: vec![],
        max_tokens: 100,
    };
    t.record_abandoned(1, &req);
    assert!(t.spend_today() > 0.009, "{}", t.spend_today());
    assert_eq!(basis_of(&t), Some("estimated"));
}

// The size rule counts the system prompt and the tool specifications, which go out with every round.
#[test]
fn the_request_size_includes_the_system_prompt_and_the_tools() {
    let mut req = ChatRequest {
        model: "m".into(),
        messages: vec![Message::system("s".repeat(1000)), Message::user("hi")],
        tools: vec![],
        max_tokens: 100,
    };
    let bare = req.sent_bytes();
    assert!(bare >= 1002);
    req.tools.push(ToolSpec {
        name: "set_timer".into(),
        description: "d".repeat(200),
        parameters: json!({"type": "object", "properties": {"seconds": {"type": "integer"}}}),
    });
    assert!(req.sent_bytes() >= bare + 200 + 9, "the spec is counted");
    let c = Completion {
        text: "x".repeat(90),
        ..Completion::default()
    };
    assert_eq!(c.received_bytes(), 90);
}

// Whole and streamed replies feed the same rules: with tokens and no cost they are priced, with neither
// they are estimated from sizes.
#[test]
fn replies_that_arrive_whole_or_streamed_are_priced_alike() {
    let priced = UsageLimits {
        llm_usd_per_mtok_in: Some(2.0),
        llm_usd_per_mtok_out: Some(8.0),
        ..UsageLimits::default()
    };
    let req = ChatRequest {
        model: "m".into(),
        messages: vec![Message::user("x".repeat(300))],
        tools: vec![],
        max_tokens: 100,
    };
    let whole = parse_completion(
        r#"{"choices":[{"message":{"content":"Hi."},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1000,"completion_tokens":500}}"#,
    )
    .unwrap();
    let mut acc = StreamAccumulator::default();
    for line in [
        r#"data: {"choices":[{"delta":{"content":"Hi."}}],"usage":null}"#,
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":null}"#,
        r#"data: {"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":500}}"#,
        "data: [DONE]",
    ] {
        acc.feed_line(line).unwrap();
    }
    let streamed = acc.finish();
    for c in [&whole, &streamed] {
        let t = UsageTracker::new(priced);
        t.record_reply(1, &req, c);
        assert!((t.spend_today() - 0.006).abs() < 1e-12);
        assert_eq!(basis_of(&t), Some("priced"));
    }

    // Streamed by a server that reports no usage at all: sizes and the built-in prices.
    let mut acc = StreamAccumulator::default();
    acc.feed_line(&format!(
        r#"data: {{"choices":[{{"delta":{{"content":"{}"}}}}]}}"#,
        "y".repeat(120)
    ))
    .unwrap();
    let silent = acc.finish();
    assert!(silent.usage.is_none());
    let t = UsageTracker::new(UsageLimits::default());
    t.record_reply(1, &req, &silent);
    // sent: 300 bytes and 16 of framing = 316 bytes = 106 tokens; got 120 bytes = 40 tokens
    let want = 106.0 * 10.0 + 40.0 * 40.0;
    assert!(
        (t.spend_today() * 1e6 - want).abs() < 1e-6,
        "{}",
        t.spend_today()
    );
    assert_eq!(basis_of(&t), Some("estimated"));

    // The same, not streamed.
    let body = parse_completion(r#"{"choices":[{"message":{"content":"ok"}}]}"#).unwrap();
    assert!(body.usage.is_none());
    let t = UsageTracker::new(capped(0.0));
    t.record_reply(1, &req, &body);
    assert_eq!(basis_of(&t), Some("estimated"));
    assert_eq!(t.check(1), Err(LimitReached::Spend));
}

// The weakest basis of the day wins, and a new day starts clean.
#[test]
fn the_basis_is_the_weakest_of_the_day_and_resets_with_it() {
    let t = UsageTracker::new(UsageLimits {
        llm_usd_per_mtok_in: Some(1.0),
        llm_usd_per_mtok_out: Some(1.0),
        ..UsageLimits::default()
    });
    t.record_sized(1, Some(&usage(Some(0.001))), 0, 0);
    assert_eq!(basis_of(&t), Some("reported"));
    t.record_sized(1, Some(&tokens(10, 10)), 0, 0);
    assert_eq!(basis_of(&t), Some("priced"));
    t.record_sized(1, None, 100, 100);
    assert_eq!(basis_of(&t), Some("estimated"));
    t.record_sized(1, Some(&usage(Some(0.001))), 0, 0);
    assert_eq!(basis_of(&t), Some("estimated"), "it does not get better");
    t.check(2).unwrap();
    assert_eq!(t.basis(), None);
}

// REQ-AST-011: turns that run side by side count into the same counters, so the cap sees all of them.
#[test]
fn clones_count_into_the_same_counters_and_see_the_same_cap() {
    let a = UsageTracker::new(capped(0.10));
    let b = a.clone();
    a.record_sized(5, Some(&usage(Some(0.06))), 0, 0);
    b.record_sized(5, Some(&usage(Some(0.05))), 0, 0);
    assert_eq!(a.requests_today(), 2);
    assert_eq!(a.check(5), Err(LimitReached::Spend));
    assert_eq!(b.check(5), Err(LimitReached::Spend));
    b.set_limits(UsageLimits::default());
    assert!(
        a.check(5).is_ok(),
        "a change of limits reaches every handle"
    );
}

#[test]
fn a_change_of_prices_applies_to_the_next_request() {
    let t = UsageTracker::new(UsageLimits::default());
    t.record_sized(1, Some(&tokens(1_000_000, 0)), 0, 0);
    assert!((t.spend_today() - 10.0).abs() < 1e-9, "built-in price");
    t.set_limits(UsageLimits {
        llm_usd_per_mtok_in: Some(1.0),
        ..UsageLimits::default()
    });
    t.record_sized(1, Some(&tokens(1_000_000, 0)), 0, 0);
    assert!((t.spend_today() - 11.0).abs() < 1e-9, "the new price");
}

#[test]
fn the_message_says_what_still_works() {
    assert!(
        LimitReached::Requests
            .to_string()
            .contains("Timers, alarms")
    );
}

#[test]
fn counters_survive_a_restart() {
    let dir = TempDir::new("usage");
    let store = JsonStore::new(dir.0.join("usage.json"), SCHEMA);
    let t = UsageTracker::new(UsageLimits::default());
    t.record_sized(7, Some(&usage(Some(0.5))), 0, 0);
    t.save(&store).unwrap();
    let back = UsageTracker::load(&store, UsageLimits::default()).unwrap();
    assert_eq!((back.requests_today(), back.spend_today()), (1, 0.5));
    assert_eq!(basis_of(&back), Some("reported"));
}

// The counters only gained a field: a file written before it existed loads, and the basis is unknown
// until the next cost.
#[test]
fn counters_from_before_the_basis_existed_still_load() {
    let old = r#"{"day":9,"requests":3,"micro_usd":2500,"history":[{"day":8,"requests":2}]}"#;
    let s: State = serde_json::from_str(old).unwrap();
    assert_eq!((s.requests, s.micro_usd, s.basis), (3, 2500, None));
    let back: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
}

// REQ-AST-011: a device with no battery clock starts in 1970. Its unknown day (0) and any day before the saved
// one must leave today's spend alone, and a real later day still rolls.
#[test]
fn an_unset_clock_at_boot_keeps_the_day_s_spend() {
    let dir = TempDir::new("usage");
    let store = JsonStore::new(dir.0.join("usage.json"), SCHEMA);
    let t = UsageTracker::new(capped(1.0));
    t.add_cost(20_000, 0.9);
    t.record_request(20_000);
    t.save(&store).unwrap();

    let back = UsageTracker::load(&store, capped(1.0)).unwrap();
    assert!(back.check(0).is_ok(), "unknown day");
    back.add_cost(0, 0.05);
    back.record_request(0);
    assert!(back.check(3).is_ok(), "a day in 1970");
    back.add_cost(3, 0.02);
    assert_eq!(back.requests_today(), 2);
    assert!(
        (back.spend_today() - 0.97).abs() < 1e-9,
        "{}",
        back.spend_today()
    );
    // The network time arrives and it is the same day: nothing is lost.
    assert!(back.check(20_000).is_ok());
    assert!((back.spend_today() - 0.97).abs() < 1e-9);
    back.add_cost(20_000, 0.03);
    assert_eq!(back.check(20_000), Err(LimitReached::Spend));
    // A real new day rolls.
    assert!(back.check(20_001).is_ok());
    assert_eq!(back.spend_today(), 0.0);
    assert_eq!(back.request_days().1[0], (20_000, 2));
}

// While the wall clock has not been set this run, nobody knows today's day: the chart's "today" must not be the
// day of the saved counters, which may be days old.
#[test]
fn the_chart_does_not_name_the_saved_day_as_today_while_the_clock_is_unset() {
    let dir = TempDir::new("usage-chart-clock");
    let store = JsonStore::new(dir.0.join("usage.json"), SCHEMA);
    let t = UsageTracker::new(UsageLimits::default());
    t.record_request(20_000);
    t.record_request(20_000);
    t.save(&store).unwrap();
    let back = UsageTracker::load(&store, UsageLimits::default()).unwrap();
    assert!(back.check(0).is_ok());
    back.record_request(0);
    let (today, days) = back.request_days();
    assert_eq!(today, 0, "the day is unknown, not the saved day");
    assert_eq!(days, vec![(20_000, 3)], "what was counted is still charted");
    // A day before the saved one is a clock that stepped back, not a known day.
    assert!(back.check(3).is_ok());
    assert_eq!(back.request_days().0, 0);
    // The clock arrives on a later day: the saved day becomes a past day and today starts clean.
    assert!(back.check(20_003).is_ok());
    assert_eq!(
        back.request_days(),
        (20_003, vec![(20_000, 3), (20_003, 0)])
    );
}

#[test]
fn requests_counted_before_any_real_day_are_charted_on_the_first_real_day() {
    let t = UsageTracker::new(UsageLimits::default());
    t.record_request(0);
    t.record_request(0);
    assert_eq!(
        t.request_days(),
        (0, vec![]),
        "no day yet, nothing to chart"
    );
    t.record_request(20_000);
    assert_eq!(t.request_days(), (20_000, vec![(20_000, 3)]));
}

#[test]
fn spend_counted_before_the_first_real_day_is_kept_on_it() {
    let t = UsageTracker::new(UsageLimits::default());
    t.add_cost(0, 0.25);
    t.record_request(0);
    t.record_request(20_000);
    assert_eq!((t.requests_today(), t.spend_today()), (2, 0.25));
}

// A reported cost is rounded up like the others: a cost under half a millionth is not free.
#[test]
fn a_tiny_reported_cost_is_rounded_up() {
    let (micro, basis) = price_request(&UsageLimits::default(), Some(&usage(Some(4e-7))), 0, 0);
    assert_eq!((micro, basis), (1, Basis::Reported));
}

// REQ-AST-011: an answer that is cut off was received by the provider and may be billed; a refusal is free.
#[test]
fn a_cut_off_answer_costs_its_prompt_and_a_refusal_costs_nothing() {
    use crate::llm::LlmError;
    let req = ChatRequest {
        model: "m".into(),
        messages: vec![Message::user("x".repeat(3000))],
        tools: vec![],
        max_tokens: 100,
    };
    let t = UsageTracker::new(UsageLimits::default());
    t.record_failed(1, &req, &LlmError::Provider("HTTP 429".into()));
    t.record_failed(1, &req, &LlmError::Transport("refused".into()));
    assert_eq!((t.requests_today(), t.spend_today()), (2, 0.0));
    t.record_failed(1, &req, &LlmError::Interrupted("cut".into()));
    assert_eq!(t.requests_today(), 3);
    assert!(t.spend_today() > 0.009, "{}", t.spend_today());
}
