use super::*;
use crate::inbox::event::EventKind;
use crate::store::testutil::TempDir;
use std::sync::atomic::{AtomicI64, Ordering};

pub(crate) struct TestClock(pub AtomicI64);
impl Clock for TestClock {
    fn monotonic_secs(&self) -> i64 {
        self.now()
    }
    fn now(&self) -> Ts {
        self.0.load(Ordering::Relaxed)
    }
}

fn core() -> (Core, Arc<TestClock>) {
    let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
    (
        Core::in_memory(clock.clone(), Adapters::builtin(), Limits::default()),
        clock,
    )
}

const STOP: &str = r#"{"session_id":"s1","cwd":"/home/a/clawdot","hook_event_name":"Stop","last_assistant_message":"Done. All tests pass."}"#;

#[test]
fn a_valid_claude_code_hook_lands_in_the_inbox() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    let out = c
        .ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    assert_eq!(out, HookOutcome::Accepted(1));
    let ev = &c.events()[0];
    assert_eq!(
        (ev.kind, ev.title.as_str(), ev.session_ref.as_deref()),
        (EventKind::Done, "clawdot finished", Some("s1"))
    );
}

#[test]
fn bad_credentials_are_refused_identically_for_unknown_endpoints_and_wrong_tokens() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    assert_eq!(
        c.ingest_hook("claude-code", "cdt_wrong", LOCALHOST_PEER, b"{}"),
        Err(HookError::Unauthorized)
    );
    assert_eq!(
        c.ingest_hook("no-such", &token, LOCALHOST_PEER, b"{}"),
        Err(HookError::Unauthorized)
    );
    assert!(c.events().is_empty());
}

#[test]
fn repeated_bad_guesses_get_rate_limited() {
    let (c, _) = core();
    c.create_endpoint("x", "generic").unwrap();
    let mut limited = false;
    for _ in 0..100 {
        if c.ingest_hook("x", "cdt_guess", LOCALHOST_PEER, b"{}") == Err(HookError::RateLimited) {
            limited = true;
            break;
        }
    }
    assert!(limited, "brute force must hit a limit");
}

#[test]
fn a_flood_from_a_valid_endpoint_is_throttled_then_recovers() {
    let (c, clock) = core();
    let (_, token) = c.create_endpoint("x", "generic").unwrap();
    let mut accepted = 0;
    let mut limited = 0;
    for i in 0..60 {
        match c.ingest_hook(
            "x",
            &token,
            LOCALHOST_PEER,
            format!(r#"{{"title":"t{i}","session_ref":"s{i}"}}"#).as_bytes(),
        ) {
            Ok(_) => accepted += 1,
            Err(HookError::RateLimited) => limited += 1,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }
    assert!(
        accepted >= 20 && limited > 0,
        "accepted {accepted}, limited {limited}"
    );
    clock.0.fetch_add(10, Ordering::Relaxed);
    assert!(
        c.ingest_hook("x", &token, LOCALHOST_PEER, br#"{"title":"later"}"#)
            .is_ok(),
        "tokens refill"
    );
}

#[test]
fn malformed_input_is_rejected_cleanly() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("x", "generic").unwrap();
    assert_eq!(
        c.ingest_hook("x", &token, LOCALHOST_PEER, b"not json"),
        Err(HookError::BadJson)
    );
    let (_, t2) = c.create_endpoint("y", "no-such-adapter").unwrap();
    assert_eq!(
        c.ingest_hook("y", &t2, LOCALHOST_PEER, b"{}"),
        Err(HookError::NoAdapter)
    );
}

#[test]
fn muted_endpoints_accept_but_do_not_queue() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("x", "generic").unwrap();
    c.update_filter("x", |f| f.muted = true);
    let out = c
        .ingest_hook("x", &token, LOCALHOST_PEER, br#"{"title":"hi"}"#)
        .unwrap();
    assert!(matches!(out, HookOutcome::Filtered(FilterReason::Muted)));
    assert!(c.events().is_empty());
}

#[test]
fn announce_reply_and_the_bridge_round_trip() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();

    let next = c.next_announcement(true, 600).unwrap();
    assert_eq!(next.event.title, "clawdot finished");
    assert!(c.next_announcement(false, 600).is_none(), "not while busy");
    c.mark_announced(&[next.event.id]);
    assert!(
        c.next_announcement(true, 600).is_none(),
        "announced events are not repeated"
    );

    let replied = c
        .reply(ReplyKind::Instruct, "keep going and add tests")
        .unwrap();
    assert_eq!(replied.event_id, next.event.id);

    // The bridge pulls it with its own credentials, and only its own.
    let pending = c
        .bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session_ref.as_deref(), Some("s1"));
    assert_eq!(pending[0].text, "keep going and add tests");
    assert!(
        c.bridge_ack("claude-code", &token, LOCALHOST_PEER, pending[0].id)
            .unwrap()
    );
    assert!(
        c.bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn saying_thanks_ill_handle_it_acknowledges_locally_and_tells_the_agent() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    let id = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[id]);
    c.reply(ReplyKind::Ack, "").unwrap();
    assert_eq!(c.events()[0].state, EventState::Acknowledged);
    assert_eq!(
        c.bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
            .unwrap()[0]
            .kind,
        ReplyKind::Ack
    );
    assert_eq!(
        c.reply(ReplyKind::Ack, ""),
        Err(ReplyError::NothingToReplyTo),
        "nothing open any more"
    );
}

#[test]
fn replying_with_nothing_announced_is_an_error_not_a_guess() {
    let (c, _) = core();
    assert_eq!(
        c.reply(ReplyKind::Instruct, "do it"),
        Err(ReplyError::NothingToReplyTo)
    );
}

#[test]
fn a_bridge_never_sees_another_endpoints_replies() {
    let (c, _) = core();
    let (_, a) = c.create_endpoint("a", "generic").unwrap();
    let (_, b) = c.create_endpoint("b", "generic").unwrap();
    c.ingest_hook(
        "a",
        &a,
        LOCALHOST_PEER,
        br#"{"title":"for a","session_ref":"sa"}"#,
    )
    .unwrap();
    let id = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[id]);
    c.reply(ReplyKind::Instruct, "x").unwrap();
    assert!(
        c.bridge_pending("b", &b, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        c.bridge_pending("a", &a, LOCALHOST_PEER, 0).unwrap().len(),
        1
    );
    assert_eq!(
        c.bridge_pending("a", &b, LOCALHOST_PEER, 0),
        Err(HookError::Unauthorized),
        "b's token is not a's"
    );
}

// REQ-CFG-028: the rate-limit state of an endpoint is not inherited either: a new endpoint under a deleted
// endpoint's name starts with a full burst, not with whatever the old agent had used up.
#[test]
fn a_recreated_endpoint_does_not_inherit_the_old_ones_rate_limit() {
    let (c, _) = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    let mut n = 0;
    while c
        .ingest_hook(
            "agent",
            &old,
            LOCALHOST_PEER,
            format!(r#"{{"title":"t{n}","session_ref":"s{n}"}}"#).as_bytes(),
        )
        .is_ok()
    {
        n += 1;
        assert!(n < 1000, "never throttled");
    }
    c.revoke_endpoint("agent").unwrap();
    let (_, new) = c.create_endpoint("agent", "generic").unwrap();
    assert!(
        c.ingest_hook("agent", &new, LOCALHOST_PEER, br#"{"title":"fresh"}"#)
            .is_ok(),
        "the new endpoint was throttled by the old one's use"
    );
}

// REQ-CFG-028: deleting an endpoint drops what was waiting for it, so an endpoint made later under the
// same name starts empty and its bridge never receives the old agent's replies.
#[test]
fn a_recreated_endpoint_inherits_no_replies_and_no_events() {
    let (c, _) = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    c.ingest_hook(
        "agent",
        &old,
        LOCALHOST_PEER,
        br#"{"title":"first","session_ref":"s1"}"#,
    )
    .unwrap();
    let first = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[first]);
    c.reply(ReplyKind::Instruct, "private to the old agent")
        .unwrap();
    c.ingest_hook(
        "agent",
        &old,
        LOCALHOST_PEER,
        br#"{"title":"second","session_ref":"s2"}"#,
    )
    .unwrap();
    assert_eq!(
        c.bridge_pending("agent", &old, LOCALHOST_PEER, 0)
            .unwrap()
            .len(),
        1
    );

    c.revoke_endpoint("agent").unwrap();
    let (_, new) = c.create_endpoint("agent", "generic").unwrap();

    assert!(
        c.bridge_pending("agent", &new, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty(),
        "the new endpoint got the old one's replies"
    );
    assert!(c.events().is_empty(), "queued events outlived the endpoint");
    assert!(
        c.next_announcement(true, 0).is_none(),
        "a revoked sender's queued event was still announced"
    );
    assert_eq!(
        c.reply(ReplyKind::Instruct, "x"),
        Err(ReplyError::NothingToReplyTo)
    );
}

#[test]
fn dismissing_and_snoozing_the_current_event() {
    let (c, clock) = core();
    let (_, token) = c.create_endpoint("x", "generic").unwrap();
    c.ingest_hook(
        "x",
        &token,
        LOCALHOST_PEER,
        br#"{"title":"one","session_ref":"1"}"#,
    )
    .unwrap();
    let id = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[id]);
    assert!(c.snooze_current(600));
    assert!(c.next_announcement(true, 0).is_none());
    clock.0.fetch_add(601, Ordering::Relaxed);
    let again = c.next_announcement(true, 0).unwrap();
    c.mark_announced(&[again.event.id]);
    assert!(c.dismiss_current());
    assert!(c.next_announcement(true, 0).is_none());
    assert!(!c.dismiss_current(), "nothing left to dismiss");
}

#[test]
fn a_huge_snooze_postpones_for_good_instead_of_overflowing() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("x", "generic").unwrap();
    c.ingest_hook(
        "x",
        &token,
        LOCALHOST_PEER,
        br#"{"title":"one","session_ref":"1"}"#,
    )
    .unwrap();
    let id = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[id]);
    assert!(c.snooze_current(i64::MAX));
    assert!(c.next_announcement(true, 0).is_none());
}

fn open_in(dir: &TempDir) -> Core {
    let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
    Core::open(
        clock,
        Adapters::builtin(),
        Limits::default(),
        Stores::in_dir(&dir.0),
    )
    .unwrap()
}

/// Which of the three state files exist after the core has written everything it owes.
fn written(c: &Core, dir: &TempDir) -> Vec<&'static str> {
    c.flush_blocking();
    let found = ["endpoints.json", "inbox.json", "replies.json"]
        .into_iter()
        .filter(|f| dir.0.join(f).exists())
        .collect();
    for f in ["endpoints.json", "inbox.json", "replies.json"] {
        let _ = std::fs::remove_file(dir.0.join(f));
    }
    found
}

#[test]
fn a_change_rewrites_only_the_files_it_changed() {
    let dir = TempDir::new("dirty");
    let c = open_in(&dir);
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    assert_eq!(written(&c, &dir), ["endpoints.json"], "a new endpoint");
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    assert_eq!(written(&c, &dir), ["inbox.json"], "a posted event");
    let id = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[id]);
    assert_eq!(written(&c, &dir), ["inbox.json"], "an announcement");
    c.reply(ReplyKind::Instruct, "go on").unwrap();
    assert_eq!(written(&c, &dir), ["replies.json"], "an instruction");
    let reply = c
        .bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
        .unwrap()[0]
        .id;
    assert!(
        c.bridge_ack("claude-code", &token, LOCALHOST_PEER, reply)
            .unwrap()
    );
    assert_eq!(written(&c, &dir), ["replies.json"], "a bridge ack");
    c.reply(ReplyKind::Ack, "").unwrap();
    assert_eq!(
        written(&c, &dir),
        ["inbox.json", "replies.json"],
        "a spoken ack"
    );
}

// REQ-INB-004: housekeeping that only prunes old handled events still saves the inbox.
#[test]
fn pruning_alone_saves_the_inbox() {
    let dir = TempDir::new("prune");
    let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
    let c = Core::open(
        clock.clone(),
        Adapters::builtin(),
        Limits::default(),
        Stores::in_dir(&dir.0),
    )
    .unwrap();
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    let id = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[id]);
    assert!(c.dismiss_current());
    written(&c, &dir);
    clock.0.fetch_add(25 * 3600, Ordering::Relaxed);
    c.maintain();
    assert!(c.events().is_empty(), "the old handled event was pruned");
    assert_eq!(written(&c, &dir), ["inbox.json"], "and the file follows");
}

#[test]
fn state_files_are_compact_json() {
    let dir = TempDir::new("compact");
    let c = open_in(&dir);
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    c.flush_blocking();
    for f in ["endpoints.json", "inbox.json"] {
        let text = std::fs::read_to_string(dir.0.join(f)).unwrap();
        assert!(!text.contains('\n'), "{f} is one line");
        assert!(!text.contains("  "), "{f} has no indentation");
        serde_json::from_str::<serde_json::Value>(&text).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn durable_resolves_once_an_accepted_event_is_on_disk() {
    let dir = TempDir::new("durable");
    let c = open_in(&dir);
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    let mark = c.save_mark();
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    c.saved_since(mark).await.unwrap();
    // No flush_blocking: awaiting durable() alone is what the HTTP layer does before it answers.
    let text = std::fs::read_to_string(dir.0.join("inbox.json")).unwrap();
    assert!(text.contains("clawdot finished"));
}

#[test]
fn state_survives_a_restart_including_pending_replies() {
    let dir = TempDir::new("core");
    let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
    let token;
    {
        let c = Core::open(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&dir.0),
        )
        .unwrap();
        token = c.create_endpoint("claude-code", "claude-code").unwrap().1;
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        let id = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[id]);
        c.reply(ReplyKind::Instruct, "carry on").unwrap();
    }
    let c = Core::open(
        clock,
        Adapters::builtin(),
        Limits::default(),
        Stores::in_dir(&dir.0),
    )
    .unwrap();
    assert_eq!(c.events().len(), 1);
    assert_eq!(
        c.bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
            .unwrap()[0]
            .text,
        "carry on"
    );
    assert!(
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .is_ok(),
        "the old token still works"
    );
}

fn peer(s: &str) -> IpAddr {
    s.parse().unwrap()
}

// REQ-SEC-004: failed attempts are counted per source address. One host that guesses until it is refused does
// not stop another host presenting the right token, nor the administrator signing in with the right password.
#[test]
fn one_hosts_bad_guesses_do_not_lock_out_another_host() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("x", "generic").unwrap();
    let hash = crate::admin::hash_password("correct horse").unwrap();
    c.admin_setup_password(peer("192.168.1.2"), hash.clone())
        .unwrap();
    let (a, b) = (peer("192.168.1.10"), peer("192.168.1.11"));
    let mut n = 0;
    while c.ingest_hook("x", "cdt_guess", a, b"{}") != Err(HookError::RateLimited) {
        n += 1;
        assert!(n <= 20, "never throttled");
    }
    assert_eq!(n, 20, "the per-address burst");
    // A is throttled everywhere: hooks, bridges, announcements, sessions, sign-in.
    assert_eq!(
        c.bridge_check("x", "cdt_guess", a),
        Err(HookError::RateLimited)
    );
    assert_eq!(
        c.announce_check("cdt_guess", a),
        Err(HookError::RateLimited)
    );
    assert_eq!(
        c.check_admin(Some("wrong"), a),
        Err(AdminError::RateLimited)
    );
    assert_eq!(c.admin_login_begin(a), Err(AdminError::RateLimited));
    // B is not touched.
    assert_eq!(
        c.ingest_hook("x", "cdt_guess", b, b"{}"),
        Err(HookError::Unauthorized)
    );
    assert!(c.ingest_hook("x", &token, b, br#"{"title":"hi"}"#).is_ok());
    assert_eq!(c.admin_login_begin(b).unwrap(), hash);
    assert!(c.admin_login_finish(b).is_ok());
}

// A good sign-in is not charged, and one's own earlier failures cost only the usual throttle.
#[test]
fn a_successful_sign_in_gives_its_attempt_back() {
    let (c, _) = core();
    let hash = crate::admin::hash_password("correct horse").unwrap();
    c.admin_setup_password(peer("192.168.1.2"), hash).unwrap();
    let a = peer("192.168.1.10");
    for _ in 0..19 {
        assert_eq!(
            c.ingest_hook("x", "cdt_guess", a, b"{}"),
            Err(HookError::Unauthorized)
        );
    }
    // One attempt left: the right password signs in, and gets it back, many times over.
    for _ in 0..5 {
        c.admin_login_begin(a).unwrap();
        c.admin_login_finish(a).unwrap();
    }
    // Spent for real: the 20th bad guess, then refused, right password or not.
    assert_eq!(
        c.ingest_hook("x", "cdt_guess", a, b"{}"),
        Err(HookError::Unauthorized)
    );
    assert_eq!(c.admin_login_begin(a), Err(AdminError::RateLimited));
}

// Many addresses together hit the backstop; one address never reaches it.
#[test]
fn the_overall_backstop_exists_and_one_address_cannot_reach_it() {
    let (c, _) = core();
    let a = peer("203.0.113.1");
    for _ in 0..1000 {
        let _ = c.ingest_hook("x", "cdt_guess", a, b"{}");
    }
    assert_eq!(
        c.ingest_hook("x", "cdt_guess", peer("203.0.113.2"), b"{}"),
        Err(HookError::Unauthorized),
        "one noisy address left the others their share"
    );
    // Enough different addresses use up the shared ceiling (100); then even a new address is refused.
    let mut refused = false;
    for i in 0..64u8 {
        let p = IpAddr::from([198, 51, 100, i]);
        for _ in 0..20 {
            if c.ingest_hook("x", "cdt_guess", p, b"{}") == Err(HookError::RateLimited) {
                refused = true;
            }
        }
    }
    assert!(refused, "the shared ceiling never applied");
    assert_eq!(
        c.ingest_hook("x", "cdt_guess", peer("198.51.100.200"), b"{}"),
        Err(HookError::RateLimited)
    );
}

#[test]
fn the_table_of_sources_stays_bounded_under_ten_thousand_addresses() {
    let (c, clock) = core();
    for i in 0..10_000u32 {
        let p = IpAddr::from([10, (i >> 16) as u8, (i >> 8) as u8, i as u8]);
        let _ = c.ingest_hook("x", "cdt_guess", p, b"{}");
        if i % 50 == 0 {
            clock.0.fetch_add(10, Ordering::Relaxed);
        }
        assert!(c.lock().auth_failures.tracked() <= c.limits().auth_fail_sources);
    }
}

#[test]
fn saved_since_reports_a_write_that_failed_and_then_one_that_did_not() {
    let dir = TempDir::new("save-outcome");
    let c = open_in(&dir);
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    c.flush_blocking();
    let blocker = crate::store::temp_path(&dir.0.join("inbox.json"));
    std::fs::create_dir(&blocker).unwrap();
    let mark = c.save_mark();
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    let r = futures_lite_block(c.saved_since(mark));
    assert_eq!(r, Err(persist::SaveError { disk_full: false }));
    assert_eq!(c.events().len(), 1, "the event is still held in memory");
    // An unrelated file's failure is not ours, and once saving works the next change reports success.
    std::fs::remove_dir(&blocker).unwrap();
    let mark = c.save_mark();
    c.ingest_hook(
        "claude-code",
        &token,
        LOCALHOST_PEER,
        STOP.replace("s1", "s2").as_bytes(),
    )
    .unwrap();
    assert_eq!(futures_lite_block(c.saved_since(mark)), Ok(()));
}

/// Run a future to its end on a throwaway runtime (these tests have none of their own).
fn futures_lite_block<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn status_reports_what_is_open() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    let st = c.status();
    // The roll-up names the endpoint, not the label the payload or adapter carries.
    assert_eq!((st[0].origin.as_str(), st[0].finished), ("claude-code", 1));
    assert_eq!(c.events()[0].source, "Claude Code");
    let said = crate::inbox::summary::status_sentence(&st);
    assert!(said.starts_with("claude-code: "), "{said}");
    c.set_endpoint_spoken_name("claude-code", "the laptop")
        .unwrap();
    let said = crate::inbox::summary::status_sentence(&c.status());
    assert!(said.starts_with("the laptop: "), "{said}");
}

#[test]
fn a_hook_payload_cannot_choose_who_the_speaker_names() {
    let (c, _) = core();
    let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
    // Every field a payload could carry that looks like an origin.
    let payload = r#"{"hook_event_name":"Stop","session_id":"s1","cwd":"/ClawDot",
        "source":"ClawDot","from":"Mum","name":"the bank","title":"ClawDot"}"#;
    c.ingest_hook("claude-code", &token, LOCALHOST_PEER, payload.as_bytes())
        .unwrap();
    let ev = c.next_announcement(true, 0).unwrap().event;
    let line = crate::inbox::summary::announcement(&ev, "", 0);
    assert!(line.starts_with("From claude-code: "), "{line}");
    assert!(!line.contains("Mum") && !line.contains("bank"), "{line}");
}

// REQ-RTV-011
#[test]
fn one_field_of_the_status_can_be_replaced_between_snapshots() {
    let (c, _) = core();
    c.patch_ui_status("realtime", serde_json::json!({"active": true}));
    assert!(c.ui_status().is_null(), "nothing before the first snapshot");
    c.set_ui_status(serde_json::json!({"version": "9", "realtime": {"active": false}}));
    c.patch_ui_status("realtime", serde_json::json!({"active": true}));
    assert_eq!(
        c.ui_status(),
        serde_json::json!({"version": "9", "realtime": {"active": true}})
    );
}

// ----- durations run on the monotonic clock (REQ-CFG-019, REQ-CFG-069, REQ-SEC-004) -----

fn stepping_core() -> (Core, Arc<crate::sched::testclock::StepClock>) {
    let clock = Arc::new(crate::sched::testclock::StepClock::at(1_800_000_000));
    let core = Core::in_memory(clock.clone(), Adapters::builtin(), Limits::default());
    (core, clock)
}

// REQ-CFG-019: a sign-in lasts 12 hours of real time, whatever the wall clock does.
#[test]
fn an_administrator_session_does_not_get_longer_or_shorter_when_the_wall_clock_steps() {
    for (step, later, alive) in [
        (-24 * 3600, crate::admin::SESSION_SECS + 1, false),
        (24 * 3600, crate::admin::SESSION_SECS + 1, false),
        (-24 * 3600, crate::admin::SESSION_SECS - 1, true),
        (24 * 3600, 60, true),
    ] {
        let (c, clock) = stepping_core();
        let hash = crate::admin::hash_password("correct horse").unwrap();
        let token = c.admin_setup_password(peer("192.168.1.2"), hash).unwrap();
        let who = peer("192.168.1.2");
        assert_eq!(c.check_admin(Some(&token), who), Ok(()));
        clock.advance(later);
        clock.step_wall(step);
        assert_eq!(
            c.check_admin(Some(&token), who).is_ok(),
            alive,
            "step {step} s, {later} s on the monotonic clock"
        );
    }
}

// REQ-CFG-069: 300 s window, 60 s cooldown; both durations.
#[test]
fn the_reset_window_and_its_cooldown_do_not_get_longer_when_the_wall_clock_steps_back() {
    let who = peer("192.168.1.9");
    // Stepped back: the window still closes after 300 s.
    let (c, clock) = stepping_core();
    c.admin_setup_password(who, crate::admin::hash_password("correct horse").unwrap())
        .unwrap();
    assert_eq!(c.admin_open_reset_window(), Ok(300));
    clock.advance(299);
    clock.step_wall(-3600);
    assert_eq!(c.admin_window_begin(who), Ok(()), "299 s: still open");
    clock.advance(1);
    assert_eq!(
        c.admin_window_begin(who),
        Err(AdminError::WindowClosed),
        "300 s: closed, though the wall clock reads 59 minutes earlier"
    );
    // The cooldown after it also counts real time: refused at 59 s, allowed at 60 s.
    clock.advance(59);
    assert_eq!(c.admin_open_reset_window(), Err(AdminError::RateLimited));
    clock.advance(1);
    clock.step_wall(-3600);
    assert_eq!(c.admin_open_reset_window(), Ok(300));
    // Stepped forward: it does not close early.
    let (c, clock) = stepping_core();
    c.admin_setup_password(who, crate::admin::hash_password("correct horse").unwrap())
        .unwrap();
    c.admin_open_reset_window().unwrap();
    clock.advance(100);
    clock.step_wall(6 * 3600);
    assert_eq!(c.admin_window_begin(who), Ok(()));
}

// REQ-SEC-004: a step of the wall clock must not hand a guesser a fresh budget.
#[test]
fn a_forward_step_of_the_wall_clock_does_not_refill_the_failed_attempt_budget() {
    let (c, clock) = stepping_core();
    let a = peer("192.168.1.10");
    let mut n = 0;
    while c.ingest_hook("x", "cdt_guess", a, b"{}") != Err(HookError::RateLimited) {
        n += 1;
        assert!(n <= 20, "never throttled");
    }
    clock.step_wall(24 * 3600);
    assert_eq!(
        c.ingest_hook("x", "cdt_guess", a, b"{}"),
        Err(HookError::RateLimited),
        "no real time has passed"
    );
    clock.advance(2);
    assert_eq!(
        c.ingest_hook("x", "cdt_guess", a, b"{}"),
        Err(HookError::Unauthorized),
        "real time refills it"
    );
}
