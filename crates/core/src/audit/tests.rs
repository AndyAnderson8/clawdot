use super::*;
use crate::store::testutil::TempDir;

fn act<'a>(tool: &'a str, tier: Tier, args: &'a Value, d: &'a str) -> Act<'a> {
    Act {
        tool,
        tier: Some(tier),
        args,
        decision: d,
        held_id: None,
        outcome: None,
        note: None,
    }
}

fn cfg(max: u64) -> AuditConfig {
    AuditConfig {
        max_file_bytes: max,
        ..AuditConfig::default()
    }
}

// REQ-SEC-014
#[test]
fn secrets_in_arguments_and_results_are_redacted() {
    let args = json!({
        "url": "https://bob:hunter2@example.com/x?api_key=abc123&room=den",
        "Authorization": "Bearer abcdefghijklmnop",
        "note": "use Bearer sk-live-1234567890abcdef please",
        "password": "p",
        "nested": {"client_secret": "zzz", "keep": "plain words"},
        "blob": "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8",
        "n": 5,
    });
    let v = redact_args(&args);
    let s = v.to_string();
    for leak in [
        "hunter2",
        "abc123",
        "abcdefghijklmnop",
        "sk-live",
        "zzz",
        "A1b2C3d4E5f6",
    ] {
        assert!(!s.contains(leak), "{leak} leaked in {s}");
    }
    assert!(s.contains("room=den"), "{s}");
    assert!(s.contains("plain words"), "{s}");
    assert_eq!(v["n"], 5);
    let r = excerpt("token=abc and cdt_0123456789abcdef and password: hunter2");
    assert!(!r.contains("abc") || r.contains(REDACTED));
    assert!(!r.contains("hunter2") && !r.contains("cdt_0123"), "{r}");
}

// REQ-SEC-011, REQ-SEC-014
#[test]
fn spoken_arguments_hide_secrets_and_shorten_long_values() {
    let args = json!({
        "note": "use Bearer abcdefghijklmnop1234",
        "api_key": "sk-live-0123456789abcdef",
        "url": "https://bob:hunter2@example.com/x",
        "body": "word ".repeat(40),
        "room": "den",
    });
    let s = spoken_args(&args).to_string();
    for leak in ["abcdefghijklmnop1234", "sk-live", "hunter2", "word word"] {
        assert!(!s.contains(leak), "{leak} leaked in {s}");
    }
    assert!(s.contains("a long value") && s.contains("den"), "{s}");
}

// REQ-SEC-014
#[test]
fn values_are_clipped_and_cleaned() {
    let long = "x ".repeat(500);
    let args =
        json!({ "text": long, "weird\nkey": "a\u{7}b\nc", "list": (0..100).collect::<Vec<_>>() });
    let v = redact_args(&args);
    assert!(v.to_string().len() <= MAX_ARGS_BYTES + 50, "{v}");
    let big = redact_args(
        &json!({"a": "y".repeat(190), "b": "z".repeat(190), "c": "q".repeat(190),
        "d": "w".repeat(190), "e": "r".repeat(190), "f": "t".repeat(190)}),
    );
    assert!(big.get("_truncated").is_some(), "{big}");
    let r = excerpt(&"é".repeat(1000));
    assert!(r.chars().count() <= MAX_VALUE_CHARS + 3);
    assert_eq!(clean_name("rm -rf /;\n"), "rm_-rf____");
    assert!(clean_name(&"a".repeat(500)).len() <= MAX_NAME_CHARS + 3);
}

// REQ-SEC-014
#[test]
fn records_are_appended_in_a_private_file_and_read_back() {
    let dir = TempDir::new("audit");
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    let args = json!({"minutes": 10});
    let t = log.next_turn();
    let a = Act {
        outcome: Some(Ok("Timer set.")),
        ..act("set_timer", Tier::Reversible, &args, decision::RAN)
    };
    let id = log.record(1000, 7, t, "voice", Some(0.002), &a).unwrap();
    let held = log
        .record(
            1001,
            7,
            t,
            "event:nas-monitor",
            None,
            &act("docker__restart", Tier::Costly, &json!({}), decision::HELD),
        )
        .unwrap();
    assert_eq!((id, held), (1, 2));
    let page = log.query(&Query::default());
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.records[0].id, 2);
    let r = &page.records[1];
    assert_eq!(
        (r.tool.as_str(), r.tier.as_str(), r.status.as_deref()),
        ("set_timer", "reversible", Some("ok"))
    );
    assert_eq!(r.turn_cost_usd, Some(0.002));
    assert_eq!(r.trigger, "voice");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.0.join("audit.jsonl"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // ids and turns continue after a restart
    drop(log);
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    assert_eq!(log.next_turn(), t + 1);
    let again = log
        .record(
            1002,
            7,
            t + 1,
            "voice",
            None,
            &act("set_timer", Tier::Reversible, &args, decision::RAN),
        )
        .unwrap();
    assert_eq!(again, 3);
}

// REQ-SEC-014
#[test]
fn the_file_rotates_and_stays_bounded() {
    let dir = TempDir::new("audit-rot");
    let log = AuditLog::open(&dir.0, cfg(2_000)).unwrap();
    let args = json!({"n": 1});
    for i in 0..200 {
        log.record(
            i,
            1,
            1,
            "voice",
            None,
            &act("set_timer", Tier::Reversible, &args, decision::RAN),
        );
    }
    let size = |n: &str| fs::metadata(dir.0.join(n)).map_or(0, |m| m.len());
    assert!(size("audit.jsonl") <= 2_000, "{}", size("audit.jsonl"));
    assert!(size("audit.1.jsonl") <= 2_000);
    assert!(size("audit.1.jsonl") > 0, "it rotated");
    let all = log.query(&Query {
        limit: Some(MAX_PAGE),
        ..Query::default()
    });
    assert!(!all.records.is_empty() && all.records.len() < 200);
    assert_eq!(all.records[0].id, 200, "the newest record survives");
    let ids: Vec<u64> = all.records.iter().map(|r| r.id).collect();
    assert!(ids.windows(2).all(|w| w[0] == w[1] + 1), "no gaps: {ids:?}");
    assert!(log.health().bytes <= 2_000);
}

// REQ-SEC-014
#[test]
fn a_damaged_or_half_written_log_is_tolerated_at_start_up() {
    let dir = TempDir::new("audit-bad");
    {
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        for _ in 0..3 {
            log.record(
                5,
                1,
                4,
                "voice",
                None,
                &act("set_timer", Tier::Reversible, &json!({}), decision::RAN),
            );
        }
    }
    // garbage in the middle, a line from the future, and a crash mid-write at the end
    let path = dir.0.join("audit.jsonl");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("{not json\n\u{0}\u{1}binary\n{\"v\":99,\"id\":1}\n{\"v\":1,\"id\":9,\"ts\":");
    fs::write(&path, text).unwrap();
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    let h = log.health();
    assert!(h.corrupt_lines >= 3, "{h:?}");
    let id = log
        .record(
            6,
            1,
            5,
            "voice",
            None,
            &act("set_timer", Tier::Reversible, &json!({}), decision::RAN),
        )
        .unwrap();
    assert_eq!(id, 4, "continues after the last good record");
    let page = log.query(&Query::default());
    assert_eq!(page.records.len(), 4);
    // the new record starts on its own line, so it is readable
    assert_eq!(page.records[0].id, 4);
    // an unreadable file (a directory in its place) is an error to report, never a panic
    let bad = TempDir::new("audit-dir");
    fs::create_dir_all(bad.0.join("audit.jsonl")).unwrap();
    let log = AuditLog::open(&bad.0, AuditConfig::default()).unwrap();
    assert!(
        log.record(
            1,
            1,
            1,
            "voice",
            None,
            &act("x", Tier::ReadOnly, &json!({}), decision::RAN)
        )
        .is_none()
    );
    assert_eq!(log.health().write_errors, 1);
}

// REQ-SEC-014
#[test]
fn paging_and_filters() {
    let log = AuditLog::in_memory(AuditConfig::default());
    let none = json!({});
    for i in 0..10_i64 {
        let (tool, tier, d, trig) = if i % 2 == 0 {
            ("set_timer", Tier::Reversible, decision::RAN, "voice")
        } else {
            (
                "docker__restart",
                Tier::Costly,
                decision::PROPOSED,
                "event:nas",
            )
        };
        log.record(100 + i, 1, 1, trig, None, &act(tool, tier, &none, d));
    }
    let p1 = log.query(&Query {
        limit: Some(4),
        ..Query::default()
    });
    assert_eq!(
        p1.records.iter().map(|r| r.id).collect::<Vec<_>>(),
        [10, 9, 8, 7]
    );
    assert_eq!(p1.next_before, Some(7));
    let p2 = log.query(&Query {
        limit: Some(4),
        before: p1.next_before,
        ..Query::default()
    });
    assert_eq!(p2.records[0].id, 6);
    let p3 = log.query(&Query {
        limit: Some(4),
        before: p2.next_before,
        ..Query::default()
    });
    assert_eq!(p3.records.len(), 2);
    assert_eq!(p3.next_before, None);
    let by = |f: &dyn Fn(&mut Query)| {
        let mut q = Query::default();
        f(&mut q);
        log.query(&q).records.len()
    };
    assert_eq!(by(&|q| q.tool = Some("docker__restart".into())), 5);
    assert_eq!(by(&|q| q.tier = Some("reversible".into())), 5);
    assert_eq!(by(&|q| q.decision = Some("proposed".into())), 5);
    assert_eq!(by(&|q| q.trigger = Some("event".into())), 5);
    assert_eq!(by(&|q| q.trigger = Some("event:nas".into())), 5);
    assert_eq!(by(&|q| q.trigger = Some("eve".into())), 0);
    assert_eq!(by(&|q| q.since = Some(107)), 3);
    assert_eq!(by(&|q| q.decision = Some("blocked:".into())), 0);
    assert_eq!(by(&|q| q.limit = Some(0)), 1, "limit is at least one");
}

// REQ-SEC-015
#[test]
fn daily_caps_count_what_ran_and_reset_with_the_day() {
    let log = AuditLog::in_memory(AuditConfig {
        policy: ActionPolicy {
            dry_run: false,
            max_actions_per_day: 3,
            max_costly_per_day: 1,
        },
        ..AuditConfig::default()
    });
    let none = json!({});
    let run = |tier, d| {
        log.record(1, 5, 1, "voice", None, &act("t", tier, &none, d));
    };
    assert_eq!(log.gate(5, Tier::Costly), Gate::Run);
    run(Tier::Costly, decision::APPROVED);
    assert_eq!(
        log.gate(5, Tier::Costly),
        Gate::Capped(CapHit::Costly(1)),
        "costly cap"
    );
    assert_eq!(log.gate(5, Tier::Reversible), Gate::Run);
    run(Tier::Reversible, decision::RAN);
    run(Tier::Reversible, decision::RAN);
    assert_eq!(
        log.gate(5, Tier::Reversible),
        Gate::Capped(CapHit::Actions(3))
    );
    // reads, held and blocked records do not count
    assert_eq!(log.gate(5, Tier::ReadOnly), Gate::Run);
    run(Tier::ReadOnly, decision::RAN);
    run(Tier::Costly, decision::HELD);
    assert_eq!(log.today(5), (3, 1));
    // a new day starts clean
    assert_eq!(log.gate(6, Tier::Costly), Gate::Run);
    assert_eq!(log.today(6), (0, 0));
}

// REQ-SEC-015
#[test]
fn the_day_s_count_survives_a_restart() {
    let dir = TempDir::new("audit-caps");
    let policy = ActionPolicy {
        max_actions_per_day: 2,
        ..ActionPolicy::default()
    };
    let c = AuditConfig {
        policy,
        ..AuditConfig::default()
    };
    {
        let log = AuditLog::open(&dir.0, c.clone()).unwrap();
        for _ in 0..2 {
            log.record(
                1,
                9,
                1,
                "voice",
                None,
                &act("t", Tier::Reversible, &json!({}), decision::RAN),
            );
        }
    }
    let log = AuditLog::open(&dir.0, c).unwrap();
    assert_eq!(
        log.gate(9, Tier::Reversible),
        Gate::Capped(CapHit::Actions(2))
    );
}

// REQ-SEC-016
#[test]
fn dry_run_gates_everything_but_reads_and_uses_no_allowance() {
    let log = AuditLog::in_memory(AuditConfig::default());
    let mut p = log.policy();
    p.dry_run = true;
    log.set_policy(p);
    assert_eq!(log.gate(1, Tier::ReadOnly), Gate::Run);
    for t in [Tier::Reversible, Tier::Costly, Tier::Destructive] {
        assert_eq!(log.gate(1, t), Gate::DryRun);
    }
    let id = log
        .record(
            1,
            1,
            1,
            "voice",
            None,
            &act("t", Tier::Costly, &json!({}), decision::DRY_RUN),
        )
        .unwrap();
    let page = log.query(&Query::default());
    assert_eq!(page.records[0].id, id);
    assert!(page.records[0].dry_run);
    assert_eq!(log.today(1), (0, 0));
}

fn ids(p: &Page) -> Vec<u64> {
    p.records.iter().map(|r| r.id).collect()
}

fn put(log: &AuditLog, n: u64) {
    let args = json!({"n": "x".repeat(150)});
    for i in 0..n {
        log.record(
            i64::try_from(i).unwrap(),
            1,
            1,
            "voice",
            None,
            &act("set_timer", Tier::Reversible, &args, decision::RAN),
        );
    }
}

// REQ-SEC-014
#[test]
fn a_page_that_ends_exactly_at_the_oldest_record_has_no_next_page() {
    let log = AuditLog::in_memory(AuditConfig::default());
    put(&log, 10);
    let page = |limit, before| {
        log.query(&Query {
            limit: Some(limit),
            before,
            ..Query::default()
        })
    };
    let p = page(10, None);
    assert_eq!((p.records.len(), p.next_before), (10, None), "all of it");
    let p = page(9, None);
    assert_eq!((ids(&p).last().copied(), p.next_before), (Some(2), Some(2)));
    let p = page(5, Some(6));
    assert_eq!((ids(&p), p.next_before), (vec![5, 4, 3, 2, 1], None));
    let p = page(4, Some(6));
    assert_eq!((ids(&p), p.next_before), (vec![5, 4, 3, 2], Some(2)));
    let p = page(5, Some(1));
    assert_eq!((p.records.len(), p.next_before), (0, None), "nothing older");
    let p = page(5, Some(100));
    assert_eq!(ids(&p), [10, 9, 8, 7, 6], "a cursor past the newest");
}

// REQ-SEC-014
#[test]
fn paging_walks_a_rotated_log_on_disk_without_a_gap_or_a_repeat() {
    let dir = TempDir::new("audit-pages");
    // Files several chunks long, so lines straddle the chunk boundaries of the backwards read.
    let log = AuditLog::open(&dir.0, cfg(20_000)).unwrap();
    put(&log, 300);
    assert!(fs::metadata(dir.0.join("audit.1.jsonl")).is_ok(), "rotated");
    assert!(fs::metadata(dir.0.join("audit.1.jsonl")).unwrap().len() > 2 * REV_CHUNK as u64);
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let p = log.query(&Query {
            limit: Some(37),
            before,
            ..Query::default()
        });
        seen.extend(ids(&p));
        match p.next_before {
            Some(b) => before = Some(b),
            None => break,
        }
    }
    assert_eq!(seen[0], 300);
    assert!(seen.windows(2).all(|w| w[0] == w[1] + 1), "{seen:?}");
    let all = log.query(&Query {
        limit: Some(MAX_PAGE),
        ..Query::default()
    });
    assert_eq!(ids(&all), seen, "one big page and many small ones agree");
}

// REQ-SEC-014
#[test]
fn a_log_out_of_order_or_with_repeated_ids_is_still_sorted_and_deduplicated() {
    let dir = TempDir::new("audit-order");
    {
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        put(&log, 5);
    }
    let path = dir.0.join("audit.jsonl");
    let lines: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    // 3, 1, 5, 2, 4, then 2 again
    let order = [2, 0, 4, 1, 3, 1];
    let text: String = order.iter().map(|i| format!("{}\n", lines[*i])).collect();
    fs::write(&path, text).unwrap();
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    let p = log.query(&Query {
        limit: Some(3),
        ..Query::default()
    });
    assert_eq!((ids(&p), p.next_before), (vec![5, 4, 3], Some(3)));
    let p = log.query(&Query::default());
    assert_eq!(ids(&p), [5, 4, 3, 2, 1]);
    assert_eq!(log.summary()["records"], 6, "the summary counts lines");
}

// REQ-SEC-014
#[test]
fn damaged_lines_are_skipped_and_counted_only_by_a_full_read() {
    let dir = TempDir::new("audit-damage");
    {
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        put(&log, 6);
    }
    let path = dir.0.join("audit.jsonl");
    let lines: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut text = String::new();
    for (i, l) in lines.iter().enumerate() {
        text.push_str(l);
        text.push('\n');
        if i == 1 || i == 3 {
            text.push_str("{broken\n\n");
        }
    }
    fs::write(&path, text).unwrap();
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    assert_eq!(log.health().corrupt_lines, 2);
    // Damage the count by hand, then see which queries refresh it.
    lock(&log.state).corrupt_lines = 99;
    let p = log.query(&Query {
        limit: Some(2),
        ..Query::default()
    });
    assert_eq!(ids(&p), [6, 5]);
    assert_eq!(log.health().corrupt_lines, 99, "stopped before the damage");
    let p = log.query(&Query::default());
    assert_eq!(ids(&p), [6, 5, 4, 3, 2, 1], "damaged lines are skipped");
    assert_eq!(log.health().corrupt_lines, 2, "a full read recounts them");
}

// REQ-CFG-063
#[test]
fn the_summary_counts_everything_and_lists_the_latest_failures_first() {
    let dir = TempDir::new("audit-summary");
    let log = AuditLog::open(&dir.0, cfg(3_000)).unwrap();
    let args = json!({});
    for i in 0..40_i64 {
        let outcome = if i % 4 == 0 { Err("boom") } else { Ok("fine") };
        let a = Act {
            outcome: Some(outcome),
            ..act("set_timer", Tier::Reversible, &args, decision::RAN)
        };
        log.record(1_000 + i, 1, 1, "voice", None, &a);
    }
    let s = log.summary();
    let kept = s["records"].as_u64().unwrap();
    assert!(
        kept > 10 && kept < 40,
        "rotation dropped the oldest: {kept}"
    );
    assert_eq!(s["by_decision"]["ran"], kept);
    assert_eq!(s["newest"], 1_039);
    assert_eq!(s["oldest"], 1_040 - kept);
    let failures = s["recent_failures"].as_array().unwrap();
    let times: Vec<i64> = failures.iter().map(|f| f["ts"].as_i64().unwrap()).collect();
    assert_eq!(times[0], 1_036, "newest failure first");
    assert!(times.windows(2).all(|w| w[0] > w[1]) && times.len() <= 10);
}

#[test]
fn lines_are_read_backwards_across_chunk_boundaries() {
    let dir = TempDir::new("audit-rev");
    let path = dir.0.join("f.txt");
    let text = "one\n\ntwo-é-two\nthree\nfour with more text\nlast without newline";
    fs::write(&path, text).unwrap();
    for chunk in [1, 2, 3, 5, 8, 4096] {
        let mut rev = RevLines::new(fs::File::open(&path).unwrap(), chunk);
        let mut got = Vec::new();
        while let Some(l) = rev.next_line() {
            got.push(String::from_utf8(l).unwrap());
        }
        let mut want: Vec<String> = text.split('\n').map(str::to_owned).collect();
        want.reverse();
        want.retain(|l| !l.is_empty());
        got.retain(|l| !l.is_empty());
        assert_eq!(got, want, "chunk {chunk}");
    }
    fs::write(&path, "").unwrap();
    let mut rev = RevLines::new(fs::File::open(&path).unwrap(), 4);
    assert_eq!(rev.next_line(), None, "an empty file");
}

// REQ-SEC-014
#[test]
fn nothing_the_model_or_an_event_chooses_can_break_the_file() {
    let log = AuditLog::in_memory(AuditConfig::default());
    let hostile = json!({"a\"b\n{\"id\":1}": "line1\n{\"v\":1,\"id\":99}\r\n"});
    let a = Act {
        tool: "x\n{\"v\":1}",
        tier: None,
        args: &hostile,
        decision: decision::INVALID,
        held_id: None,
        outcome: Some(Err("boom\n{\"id\":5}")),
        note: None,
    };
    log.record(1, 1, 1, "event:ev\nil", None, &a).unwrap();
    let page = log.query(&Query::default());
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].tier, "unknown");
    assert!(!page.records[0].tool.contains('\n'));
    assert_eq!(log.health().corrupt_lines, 0);
}

// REQ-SEC-016: the caps must hold when the log cannot be written (a full or read-only disk).
#[test]
fn an_action_counts_toward_the_caps_even_when_its_record_cannot_be_written() {
    let dir = TempDir::new("audit-unwritable");
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    let act = |tool: &'static str| Act {
        tool,
        tier: Some(Tier::Costly),
        args: &Value::Null,
        decision: decision::RAN,
        held_id: None,
        outcome: Some(Ok("ok")),
        note: None,
    };
    assert!(
        log.record(100, 1, 1, "voice", None, &act("first"))
            .is_some()
    );
    assert_eq!(log.today(1), (1, 1));
    // Put a directory where the log file is, so the next append fails.
    for entry in std::fs::read_dir(&dir.0).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
    }
    assert!(
        log.record(101, 1, 1, "voice", None, &act("second"))
            .is_none()
    );
    assert_eq!(log.today(1), (2, 2), "the action ran, so it counts");
    assert!(log.health().write_errors >= 1);
}

// REQ-SEC-011, REQ-SEC-014: a name is a secret by its whole words, so ordinary names are heard and recorded.
#[test]
fn a_key_is_secret_by_its_words_not_by_letters_it_happens_to_contain() {
    for secret in [
        "token",
        "api_key",
        "apiKey",
        "APIKey",
        "X-Api-Key",
        "x-auth-token",
        "Authorization",
        "auth",
        "client_secret",
        "clientSecret",
        "accesstoken",
        "refresh_tokens",
        "password",
        "passwordConfirm",
        "private_key",
        "sessionId",
        "session",
        "otp",
        "cookie",
        "credentials",
        "signature",
        "jwt",
        "keys",
    ] {
        assert!(is_secret_name(secret), "{secret} names a secret");
    }
    for plain in [
        "keyword",
        "keywords",
        "author",
        "authors",
        "authored_by",
        "monkey",
        "turkey",
        "hotkeys",
        "private",
        "privately",
        "tokenize",
        "passenger",
        "address",
        "status",
        "title",
        "text",
        "",
    ] {
        assert!(!is_secret_name(plain), "{plain} does not name a secret");
    }
    // The record and the read-back keep what is not a secret.
    let v = redact_args(&json!({ "author": "Ada", "keywords": "tea, kettle", "api_key": "abc" }));
    assert_eq!(v["author"], "Ada");
    assert_eq!(v["keywords"], "tea, kettle");
    assert_eq!(v["api_key"], REDACTED);
}

fn ran(log: &AuditLog, day: i64, tier: Tier, n: u32) {
    for _ in 0..n {
        log.record(
            1,
            day,
            1,
            "voice",
            None,
            &act("t", tier, &json!({}), decision::RAN),
        );
    }
}

fn counts_file(dir: &TempDir) -> JsonStore {
    JsonStore::new(dir.0.join("audit-counts.json"), COUNTS_SCHEMA)
}

// REQ-SEC-015, REQ-SEC-016: a day busy enough to rotate the log twice must not loosen the caps at restart.
#[test]
fn the_day_s_count_survives_a_restart_after_the_log_rotated_it_away() {
    let dir = TempDir::new("audit-counts-rotated");
    {
        let log = AuditLog::open(&dir.0, cfg(1_500)).unwrap();
        ran(&log, 5, Tier::Reversible, 30);
        ran(&log, 5, Tier::Costly, 30);
        assert_eq!(log.today(5), (60, 30));
        let kept = log
            .query(&Query {
                limit: Some(MAX_PAGE),
                ..Query::default()
            })
            .records
            .len();
        assert!(kept < 60, "the log should have forgotten some ({kept})");
    }
    let log = AuditLog::open(&dir.0, cfg(1_500)).unwrap();
    assert_eq!(log.today(5), (60, 30));
}

// REQ-SEC-015: no counts file (an older version, or deleted) means the recount from the log, as before.
#[test]
fn a_missing_or_damaged_counts_file_falls_back_to_the_log() {
    let dir = TempDir::new("audit-counts-fallback");
    {
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        ran(&log, 5, Tier::Costly, 3);
    }
    std::fs::remove_file(dir.0.join("audit-counts.json")).unwrap();
    assert_eq!(
        AuditLog::open(&dir.0, AuditConfig::default())
            .unwrap()
            .today(5),
        (3, 3)
    );
    std::fs::write(dir.0.join("audit-counts.json"), b"{ not json").unwrap();
    assert_eq!(
        AuditLog::open(&dir.0, AuditConfig::default())
            .unwrap()
            .today(5),
        (3, 3)
    );
}

// REQ-SEC-015: when both exist the larger count wins, per figure; another day's file is ignored.
#[test]
fn the_larger_of_the_file_and_the_log_is_used() {
    let dir = TempDir::new("audit-counts-larger");
    {
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        ran(&log, 5, Tier::Costly, 3);
    }
    let store = counts_file(&dir);
    let save = |day, actions, costly| {
        store
            .save(&SavedCounts {
                day,
                actions,
                costly,
            })
            .unwrap();
    };
    save(5, 50, 1);
    let open = || AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    assert_eq!(open().today(5), (50, 3));
    save(5, 1, 40);
    assert_eq!(open().today(5), (3, 40));
    save(4, 99, 99);
    assert_eq!(open().today(5), (3, 3));
}

// REQ-SEC-016: the background writer carries the file when the core has one.
#[test]
fn the_counts_go_through_the_background_writer_when_there_is_one() {
    let dir = TempDir::new("audit-counts-writer");
    let writer = Arc::new(Writer::start().unwrap());
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    log.set_writer(Some(Arc::clone(&writer)));
    ran(&log, 5, Tier::Reversible, 2);
    writer.flush_blocking();
    let saved: SavedCounts = counts_file(&dir).load().unwrap();
    assert_eq!((saved.day, saved.actions, saved.costly), (5, 2, 0));
}

// REQ-SEC-016: a counts file that cannot be written does not turn the caps off, and the log is still written.
#[test]
fn an_unwritable_counts_file_still_counts() {
    let dir = TempDir::new("audit-counts-unwritable");
    let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
    ran(&log, 5, Tier::Costly, 1);
    let path = dir.0.join("audit-counts.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    ran(&log, 5, Tier::Costly, 1);
    assert_eq!(log.today(5), (2, 2));
    assert_eq!(log.query(&Query::default()).records.len(), 2);
}

// REQ-SEC-015: a device with no battery clock boots in 1970 (day 0 here). The caps must not reset for that.
#[test]
fn an_unset_clock_does_not_reset_the_daily_caps() {
    let dir = TempDir::new("audit-clock");
    let c = AuditConfig {
        policy: ActionPolicy {
            max_actions_per_day: 2,
            ..ActionPolicy::default()
        },
        ..AuditConfig::default()
    };
    let none = json!({});
    {
        let log = AuditLog::open(&dir.0, c.clone()).unwrap();
        for _ in 0..2 {
            log.record(
                1,
                20_000,
                1,
                "voice",
                None,
                &act("t", Tier::Reversible, &none, decision::RAN),
            );
        }
    }
    let log = AuditLog::open(&dir.0, c).unwrap();
    // Booted with the clock unset (day 0), or set to a day in 1970.
    for day in [0, 3] {
        assert_eq!(
            log.gate(day, Tier::Reversible),
            Gate::Capped(CapHit::Actions(2)),
            "day {day}"
        );
    }
    // The clock arrives on the same day: still capped. A real new day starts clean.
    assert_eq!(
        log.gate(20_000, Tier::Reversible),
        Gate::Capped(CapHit::Actions(2))
    );
    assert_eq!(log.gate(20_001, Tier::Reversible), Gate::Run);
}

#[test]
fn actions_run_before_the_clock_is_set_count_on_the_first_real_day() {
    let log = AuditLog::in_memory(AuditConfig {
        policy: ActionPolicy {
            max_actions_per_day: 2,
            ..ActionPolicy::default()
        },
        ..AuditConfig::default()
    });
    let none = json!({});
    let ran = act("t", Tier::Reversible, &none, decision::RAN);
    log.record(1, 0, 1, "voice", None, &ran);
    assert_eq!(log.today(0), (1, 0));
    assert_eq!(log.today(20_000), (1, 0));
}
