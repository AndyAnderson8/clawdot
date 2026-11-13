use super::*;

#[tokio::test(flavor = "current_thread")]
async fn a_plain_answer_is_returned_and_remembered() {
    let b = Script::new(vec![text("The capital is Canberra.")]);
    let (mut h, mut c, mut u) = (
        Host::default(),
        conv(),
        UsageTracker::new(UsageLimits::default()),
    );
    let r = turn(&b, &mut h, &mut c, &mut u, "capital of australia").await;
    assert_eq!(
        (r.speech.as_str(), r.end.clone(), r.model_requests),
        ("The capital is Canberra.", TurnEnd::Answered, 1)
    );
    assert_eq!(c.history().len(), 2);
    let req = &b.requests()[0];
    assert_eq!(
        (req.model.as_str(), req.max_tokens, req.messages[0].role),
        ("primary", 300, Role::System)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_tool_call_runs_then_the_model_answers_with_the_result() {
    let b = Script::new(vec![
        calls(&[("c1", "set_timer", r#"{"seconds":600}"#)]),
        text("Timer set for ten minutes."),
    ]);
    let (mut h, mut c, mut u) = (
        Host::default(),
        conv(),
        UsageTracker::new(UsageLimits::default()),
    );
    let r = turn(&b, &mut h, &mut c, &mut u, "ten minute timer").await;
    assert_eq!(
        (r.speech.as_str(), r.tool_calls, r.model_requests),
        ("Timer set for ten minutes.", 1, 2)
    );
    assert_eq!(
        h.ran,
        vec![("set_timer".to_owned(), json!({"seconds":600}))]
    );
    let second = &b.requests()[1];
    let last = second.messages.last().unwrap();
    assert_eq!(
        (
            last.role,
            last.content.as_str(),
            last.tool_call_id.as_deref()
        ),
        (Role::Tool, "set_timer ok", Some("c1"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bad_tool_calls_become_errors_the_model_can_see() {
    let b = Script::new(vec![
        calls(&[
            ("a", "set_timer", "{not json"),
            ("b", "nonexistent", "{}"),
            ("c", "set_timer", "{}"),
        ]),
        text("Sorry, that did not work."),
    ]);
    let (mut h, mut c, mut u) = (
        Host::default(),
        conv(),
        UsageTracker::new(UsageLimits::default()),
    );
    turn(&b, &mut h, &mut c, &mut u, "x").await;
    let msgs = &b.requests()[1].messages;
    let tool_msgs: Vec<&str> = msgs
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_str())
        .collect();
    assert!(tool_msgs[0].contains("not valid JSON"));
    assert!(tool_msgs[1].contains("no tool called `nonexistent`"));
    assert!(tool_msgs[2].starts_with("Error: seconds is required"));
    assert_eq!(h.ran.len(), 1, "only the well-formed call reached the tool");
}

#[tokio::test(flavor = "current_thread")]
async fn the_fallback_model_is_used_when_the_primary_fails() {
    let b = Script::new(vec![
        Err(LlmError::Provider("overloaded".into())),
        text("From the backup."),
    ]);
    let (mut h, mut c, mut u) = (
        Host::default(),
        conv(),
        UsageTracker::new(UsageLimits::default()),
    );
    let config = TurnConfig {
        fallback_model: Some("backup".into()),
        ..cfg()
    };
    let r = user_turn(&b, inputs_cfg(&mut h, &mut c, &config, &mut u, 1000), "hi").await;
    assert_eq!(r.speech, "From the backup.");
    let models: Vec<_> = b.requests().iter().map(|r| r.model.clone()).collect();
    assert_eq!(models, vec!["primary", "backup"]);
    assert_eq!(u.requests_today(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn when_every_model_fails_the_turn_says_so() {
    let b = Script::new(vec![Err(LlmError::Transport("down".into()))]);
    let (mut h, mut c, mut u) = (
        Host::default(),
        conv(),
        UsageTracker::new(UsageLimits::default()),
    );
    let r = turn(&b, &mut h, &mut c, &mut u, "hi").await;
    assert_eq!(r.speech, "Sorry, I couldn't reach the language model.");
    assert!(matches!(r.end, TurnEnd::Failed(LlmError::Transport(_))));
}

#[tokio::test(flavor = "current_thread")]
async fn history_is_bounded_and_never_starts_with_a_dangling_tool_result() {
    let replies: Vec<_> = (0..30)
        .flat_map(|i| {
            [
                calls(&[("c", "set_timer", r#"{"seconds":1}"#)]),
                text(&format!("answer {i}")),
            ]
        })
        .collect();
    let b = Script::new(replies);
    let (mut h, mut u) = (Host::default(), UsageTracker::new(UsageLimits::default()));
    let mut c = Conversation::new("s", 6, 20_000, 600);
    for i in 0..30 {
        user_turn(
            &b,
            inputs(&mut h, &mut c, &mut u, 1000 + i),
            &format!("q{i}"),
        )
        .await;
        assert!(c.history().len() <= 6 + 4, "bounded: {}", c.history().len());
        assert_eq!(
            c.history()[0].role,
            Role::User,
            "anchored at a user message"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_idle_conversation_is_forgotten() {
    let b = Script::new(vec![text("one"), text("two")]);
    let (mut h, mut c, mut u) = (
        Host::default(),
        conv(),
        UsageTracker::new(UsageLimits::default()),
    );
    user_turn(&b, inputs(&mut h, &mut c, &mut u, 1000), "a").await;
    user_turn(&b, inputs(&mut h, &mut c, &mut u, 1000 + 601), "b").await;
    assert_eq!(c.history().len(), 2, "the first exchange expired");
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_tool_results_are_clipped() {
    struct Big;
    impl ToolHost for Big {
        fn tool_specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "dump".into(),
                description: String::new(),
                parameters: json!({}),
            }]
        }
        fn risk(&self, _: &str, _: &Value) -> Risk {
            Risk::fixed(Tier::ReadOnly)
        }
        async fn call_tool(&mut self, _: &str, _: &Value) -> Result<String, String> {
            Ok("é".repeat(5_000))
        }
    }
    let b = Script::new(vec![calls(&[("c", "dump", "{}")]), text("ok")]);
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    user_turn(
        &b,
        TurnInputs {
            cfg: &TurnConfig {
                summarize_over: 0,
                ..cfg()
            },
            ..inputs(&mut Big, &mut c, &mut u, 1000)
        },
        "dump it",
    )
    .await;
    let tool = b.requests()[1]
        .messages
        .iter()
        .find(|m| m.role == Role::Tool)
        .unwrap()
        .content
        .clone();
    assert!(tool.len() < 2_100 && tool.ends_with("(cut)"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_host_is_told_the_users_words_at_the_start_of_every_turn() {
    #[derive(Default)]
    struct Spy(Vec<String>);
    impl ToolHost for Spy {
        fn tool_specs(&self) -> Vec<ToolSpec> {
            Vec::new()
        }
        fn risk(&self, _: &str, _: &Value) -> Risk {
            Risk::fixed(Tier::ReadOnly)
        }
        fn begin_turn(&mut self, user_text: &str) {
            self.0.push(user_text.to_owned());
        }
        async fn call_tool(&mut self, _: &str, _: &Value) -> Result<String, String> {
            Ok(String::new())
        }
    }
    let b = Script::new(vec![text("a"), text("b")]);
    let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
    let mut spy = Spy::default();
    user_turn(&b, inputs(&mut spy, &mut c, &mut u, 1000), "one").await;
    user_turn(&b, inputs(&mut spy, &mut c, &mut u, 1001), "two").await;
    assert_eq!(spy.0, ["one", "two"]);
}

/// Calls without a usable id (empty, repeated, or already in the history) are given fresh ones before they
/// enter the history, so each result pairs with exactly one call.
#[tokio::test(flavor = "current_thread")]
async fn calls_with_empty_or_repeated_ids_get_unique_ones_in_the_history() {
    let mut h = Lab::default();
    let b = Script::new(vec![
        calls(&[("", "act", "{}"), ("", "act", "{}"), ("dup", "act", "{}")]),
        calls(&[("dup", "act", "{}"), ("dup", "act", "{}")]),
        text("ok"),
    ]);
    let mut c = conv();
    lturn(&TurnCtl::default(), &b, &mut h, &mut c, &cfg(), "go").await;
    let ids: Vec<String> = c
        .history()
        .iter()
        .flat_map(|m| m.tool_calls.iter().map(|t| t.id.clone()))
        .collect();
    assert_eq!(ids.len(), 5);
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 5, "every call id is unique: {ids:?}");
    assert!(ids.iter().all(|i| !i.is_empty()));
    assert_eq!(ids[2], "dup", "the first use of a model id is kept");
    let results: Vec<&str> = c
        .history()
        .iter()
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    assert_eq!(results.len(), 5);
    for id in &ids {
        assert_eq!(results.iter().filter(|r| *r == id).count(), 1, "{id}");
    }
}
