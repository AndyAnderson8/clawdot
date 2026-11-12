use super::*;
use crate::inbox::endpoint::QuietHours;
use crate::store::testutil::TempDir;

fn mapped(kind: EventKind, priority: Priority, title: &str, session: Option<&str>) -> Mapped {
    Mapped {
        source: "Claude Code".into(),
        kind,
        title: title.into(),
        text: format!("text of {title}"),
        status: None,
        priority,
        session_ref: session.map(str::to_owned),
        no_reply: false,
        ttl_secs: None,
    }
}

fn setup() -> (Inbox, Endpoints) {
    let mut eps = Endpoints::default();
    eps.create("cc", "claude-code", 0).unwrap();
    (Inbox::new(InboxLimits::default()), eps)
}

fn ep(eps: &Endpoints) -> Endpoint {
    eps.get("cc").unwrap().clone()
}

#[test]
fn events_are_queued_in_order_with_increasing_ids() {
    let (mut inbox, eps) = setup();
    let a = inbox.ingest(
        10,
        &ep(&eps),
        mapped(EventKind::Done, Priority::Normal, "a", None),
    );
    let b = inbox.ingest(
        11,
        &ep(&eps),
        mapped(EventKind::Done, Priority::Normal, "b", None),
    );
    assert_eq!((a, b), (Ingest::Accepted(1), Ingest::Accepted(2)));
    assert_eq!(inbox.events().len(), 2);
}

#[test]
fn endpoint_filters_drop_unwanted_events() {
    let (mut inbox, mut eps) = setup();
    eps.get_mut("cc").unwrap().filter.min_priority = Priority::Normal;
    eps.get_mut("cc").unwrap().filter.ignore_kinds = vec![EventKind::Question];
    let e = ep(&eps);
    assert_eq!(
        inbox.ingest(1, &e, mapped(EventKind::Done, Priority::Low, "x", None)),
        Ingest::Filtered(FilterReason::BelowPriority)
    );
    assert_eq!(
        inbox.ingest(
            1,
            &e,
            mapped(EventKind::Question, Priority::Normal, "x", None)
        ),
        Ingest::Filtered(FilterReason::IgnoredKind)
    );
    eps.get_mut("cc").unwrap().filter.muted = true;
    assert_eq!(
        inbox.ingest(
            1,
            &ep(&eps),
            mapped(EventKind::Done, Priority::Urgent, "x", None)
        ),
        Ingest::Filtered(FilterReason::Muted)
    );
    assert!(inbox.events().is_empty());
}

#[test]
fn a_burst_of_turn_end_hooks_from_one_session_becomes_one_event() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    let first = inbox.ingest(
        100,
        &e,
        mapped(EventKind::Done, Priority::Normal, "t1", Some("s1")),
    );
    let second = inbox.ingest(
        110,
        &e,
        mapped(EventKind::Done, Priority::Normal, "t2", Some("s1")),
    );
    assert_eq!((first, second), (Ingest::Accepted(1), Ingest::Coalesced(1)));
    assert_eq!(inbox.events().len(), 1);
    assert_eq!(
        inbox.get(1).unwrap().title,
        "t2",
        "keeps the latest content"
    );
    // A different session, or outside the window, is a new event.
    assert_eq!(
        inbox.ingest(
            111,
            &e,
            mapped(EventKind::Done, Priority::Normal, "o", Some("s2"))
        ),
        Ingest::Accepted(2)
    );
    assert_eq!(
        inbox.ingest(
            500,
            &e,
            mapped(EventKind::Done, Priority::Normal, "late", Some("s1"))
        ),
        Ingest::Accepted(3)
    );
}

#[test]
fn announced_events_are_not_merged_into() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    inbox.ingest(
        100,
        &e,
        mapped(EventKind::Done, Priority::Normal, "t1", Some("s1")),
    );
    inbox.mark_announced(&[1]);
    assert_eq!(
        inbox.ingest(
            105,
            &e,
            mapped(EventKind::Done, Priority::Normal, "t2", Some("s1"))
        ),
        Ingest::Accepted(2)
    );
}

#[test]
fn coalescing_keeps_the_higher_priority() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    inbox.ingest(
        100,
        &e,
        mapped(EventKind::Done, Priority::Low, "a", Some("s")),
    );
    inbox.ingest(
        101,
        &e,
        mapped(EventKind::Done, Priority::Urgent, "b", Some("s")),
    );
    assert_eq!(inbox.get(1).unwrap().priority, Priority::Urgent);
}

#[test]
fn the_queue_is_bounded_and_evicts_the_least_important_first() {
    let (mut inbox, eps) = setup();
    inbox.limits.max_events = 3;
    let e = ep(&eps);
    inbox.ingest(
        1,
        &e,
        mapped(EventKind::Done, Priority::Urgent, "urgent", Some("a")),
    );
    inbox.ingest(
        2,
        &e,
        mapped(EventKind::Info, Priority::Low, "low", Some("b")),
    );
    inbox.ingest(
        3,
        &e,
        mapped(EventKind::Done, Priority::Normal, "normal", Some("c")),
    );
    inbox.ingest(
        4,
        &e,
        mapped(EventKind::Done, Priority::Normal, "newest", Some("d")),
    );
    let titles: Vec<_> = inbox.events().iter().map(|x| x.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["urgent", "normal", "newest"],
        "the low-priority one went"
    );
}

fn two_endpoints() -> (Inbox, Endpoint, Endpoint) {
    let mut eps = Endpoints::default();
    eps.create("quiet", "generic", 0).unwrap();
    eps.create("flood", "generic", 0).unwrap();
    (
        Inbox::new(InboxLimits::default()),
        eps.get("quiet").unwrap().clone(),
        eps.get("flood").unwrap().clone(),
    )
}

fn post(inbox: &mut Inbox, ep: &Endpoint, n: u64, text_len: usize) {
    let mut m = mapped(
        EventKind::Done,
        Priority::Normal,
        &format!("{}-{n}", ep.name),
        Some(&format!("{}-s{n}", ep.name)),
    );
    m.text = "x".repeat(text_len);
    inbox.ingest(i64::try_from(n).unwrap(), ep, m);
}

fn held_by(inbox: &Inbox, name: &str) -> usize {
    inbox.events().iter().filter(|e| e.endpoint == name).count()
}

#[test]
fn one_endpoint_cannot_flood_another_out_of_the_queue() {
    let (mut inbox, quiet, flood) = two_endpoints();
    inbox.limits.max_events = 8;
    post(&mut inbox, &quiet, 1, 10);
    post(&mut inbox, &quiet, 2, 10);
    for n in 10..100 {
        post(&mut inbox, &flood, n, 10);
    }
    assert_eq!(
        held_by(&inbox, "quiet"),
        2,
        "the quiet endpoint's events stay"
    );
    assert_eq!(inbox.events().len(), 8, "the flood fills the rest");
    assert_eq!(
        inbox.events().last().map(|e| e.title.as_str()),
        Some("flood-99"),
        "and the newest of its own survives"
    );
}

#[test]
fn the_byte_budget_is_shared_fairly_too() {
    let (mut inbox, quiet, flood) = two_endpoints();
    inbox.limits.max_bytes = 4_000;
    post(&mut inbox, &quiet, 1, 300);
    for n in 10..60 {
        post(&mut inbox, &flood, n, 300);
    }
    assert_eq!(held_by(&inbox, "quiet"), 1);
    assert!(inbox.total_bytes() <= 4_000);
}

#[test]
fn past_half_the_queue_an_endpoint_competes_on_the_old_rules() {
    let (mut inbox, a, b) = two_endpoints();
    inbox.limits.max_events = 4;
    for n in 1..=3 {
        post(&mut inbox, &a, n, 10);
    }
    post(&mut inbox, &b, 10, 10);
    post(&mut inbox, &b, 11, 10);
    // Three from a, two from b, room for four: the endpoint holding more pays, and its oldest goes.
    assert_eq!((held_by(&inbox, "quiet"), held_by(&inbox, "flood")), (2, 2));
    assert!(inbox.events().iter().all(|e| e.title != "quiet-1"));
}

#[test]
fn handled_events_are_evicted_before_unheard_ones() {
    let (mut inbox, eps) = setup();
    inbox.limits.max_events = 2;
    let e = ep(&eps);
    inbox.ingest(
        1,
        &e,
        mapped(EventKind::Done, Priority::Normal, "old-handled", Some("a")),
    );
    inbox.dismiss(1);
    inbox.ingest(
        2,
        &e,
        mapped(EventKind::Done, Priority::Normal, "unheard", Some("b")),
    );
    inbox.ingest(
        3,
        &e,
        mapped(EventKind::Done, Priority::Normal, "newer", Some("c")),
    );
    let titles: Vec<_> = inbox.events().iter().map(|x| x.title.as_str()).collect();
    assert_eq!(titles, vec!["unheard", "newer"]);
}

#[test]
fn the_byte_budget_is_enforced_too() {
    let (mut inbox, eps) = setup();
    inbox.limits.max_bytes = 2_000;
    let e = ep(&eps);
    for i in 0..20 {
        let mut m = mapped(
            EventKind::Done,
            Priority::Normal,
            &format!("e{i}"),
            Some(&format!("s{i}")),
        );
        m.text = "x".repeat(500);
        inbox.ingest(i, &e, m);
    }
    assert!(
        inbox.total_bytes() <= 2_000,
        "{} bytes",
        inbox.total_bytes()
    );
    assert!(!inbox.events().is_empty());
}

#[test]
fn announcement_order_is_urgent_then_blocked_then_oldest() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    inbox.ingest(
        1,
        &e,
        mapped(EventKind::Done, Priority::Normal, "done-old", Some("a")),
    );
    inbox.ingest(
        2,
        &e,
        mapped(
            EventKind::NeedsInput,
            Priority::Normal,
            "blocked",
            Some("b"),
        ),
    );
    inbox.ingest(
        3,
        &e,
        mapped(EventKind::Done, Priority::Urgent, "urgent-done", Some("c")),
    );
    let plan = inbox.plan(10, true, 600, &eps).unwrap();
    assert_eq!(inbox.get(plan.lead).unwrap().title, "urgent-done");
    assert_eq!(plan.rest.len(), 2);
    assert_eq!(inbox.get(plan.rest[0]).unwrap().title, "blocked");
    assert_eq!(inbox.get(plan.rest[1]).unwrap().title, "done-old");
}

#[test]
fn nothing_is_announced_while_the_device_is_busy() {
    let (mut inbox, eps) = setup();
    inbox.ingest(
        1,
        &ep(&eps),
        mapped(EventKind::Done, Priority::Urgent, "x", None),
    );
    assert!(inbox.plan(10, false, 600, &eps).is_none());
    assert!(inbox.plan(10, true, 600, &eps).is_some());
}

#[test]
fn quiet_hours_hold_back_everything_but_urgent() {
    let (mut inbox, mut eps) = setup();
    eps.get_mut("cc").unwrap().filter.quiet_hours = Some(QuietHours {
        start: 22 * 60,
        end: 7 * 60,
    });
    let e = ep(&eps);
    inbox.ingest(
        1,
        &e,
        mapped(EventKind::Done, Priority::Normal, "normal", Some("a")),
    );
    let night = 23 * 60;
    assert!(inbox.plan(10, true, night, &eps).is_none());
    inbox.ingest(
        2,
        &e,
        mapped(EventKind::NeedsInput, Priority::Urgent, "urgent", Some("b")),
    );
    let plan = inbox.plan(10, true, night, &eps).unwrap();
    assert_eq!(inbox.get(plan.lead).unwrap().title, "urgent");
    assert!(plan.rest.is_empty());
    assert!(
        inbox.plan(10, true, 12 * 60, &eps).unwrap().rest.len() == 1,
        "normal is released in the morning"
    );
}

// REQ-INB-040
#[test]
fn the_device_wide_window_holds_back_everything_but_urgent_and_can_be_cleared() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    inbox.ingest(
        1,
        &e,
        mapped(EventKind::Done, Priority::Normal, "normal", Some("a")),
    );
    let night = 23 * 60;
    assert!(inbox.plan(10, true, night, &eps).is_some(), "no window yet");
    inbox.set_quiet(Some(QuietHours {
        start: 22 * 60,
        end: 7 * 60,
    }));
    assert!(inbox.plan(10, true, night, &eps).is_none(), "held at night");
    assert!(
        inbox.plan(10, true, 3 * 60, &eps).is_none(),
        "the window spans midnight"
    );
    inbox.ingest(
        2,
        &e,
        mapped(EventKind::NeedsInput, Priority::Urgent, "urgent", Some("b")),
    );
    let plan = inbox.plan(10, true, night, &eps).unwrap();
    assert_eq!(inbox.get(plan.lead).unwrap().title, "urgent");
    assert!(plan.rest.is_empty(), "the normal one waits");
    assert_eq!(
        inbox.plan(10, true, 12 * 60, &eps).unwrap().rest.len(),
        1,
        "released after the window"
    );
    inbox.set_quiet(None);
    assert_eq!(
        inbox.plan(10, true, night, &eps).unwrap().rest.len(),
        1,
        "cleared at once"
    );
}

#[test]
fn a_reply_goes_to_the_event_spoken_last_not_the_one_that_arrived_last() {
    // REQ-INB-008: an older event spoken after a newer urgent one is what the person is answering.
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    let Ingest::Accepted(old) = inbox.ingest(
        1,
        &e,
        mapped(EventKind::Done, Priority::Normal, "old", Some("a")),
    ) else {
        panic!("accepted")
    };
    let Ingest::Accepted(urgent) = inbox.ingest(
        2,
        &e,
        mapped(EventKind::Question, Priority::Urgent, "urgent", Some("b")),
    ) else {
        panic!("accepted")
    };
    assert_eq!(inbox.plan(3, true, 0, &eps).unwrap().lead, urgent);
    inbox.mark_announced(&[urgent]);
    assert_eq!(inbox.reply_target().unwrap().id, urgent);
    inbox.mark_announced(&[old]);
    assert_eq!(
        inbox.reply_target().unwrap().id,
        old,
        "the older event was spoken last"
    );
    // Speaking the urgent one again (its details, a repeat) makes it the target again.
    inbox.mark_announced(&[urgent]);
    assert_eq!(inbox.reply_target().unwrap().id, urgent);
    // Once it is answered, the reply target is the one spoken before it.
    inbox.acknowledge(urgent);
    assert_eq!(inbox.reply_target().unwrap().id, old);
    // A snoozed event that comes back is the target when it is spoken again.
    let Ingest::Accepted(newer) = inbox.ingest(
        4,
        &e,
        mapped(EventKind::Done, Priority::Normal, "newer", Some("c")),
    ) else {
        panic!("accepted")
    };
    inbox.snooze(old, 10);
    inbox.mark_announced(&[newer]);
    inbox.mark_announced(&[old]);
    assert_eq!(inbox.reply_target().unwrap().id, old);
}

#[test]
fn an_inbox_saved_before_the_spoken_order_falls_back_to_arrival_order() {
    let old = r#"{"next_id":2,"events":[
        {"id":1,"endpoint":"cc","source":"s","kind":"done","title":"a","text":"x","priority":"normal",
         "received_at":1,"state":{"state":"announced"}},
        {"id":2,"endpoint":"cc","source":"s","kind":"done","title":"b","text":"x","priority":"normal",
         "received_at":2,"state":{"state":"announced"}}]}"#;
    let state: InboxState = serde_json::from_str(old).unwrap();
    let inbox = Inbox {
        state,
        limits: InboxLimits::default(),
        quiet: None,
    };
    assert_eq!(inbox.reply_target().unwrap().id, 2);
    assert!(
        !serde_json::to_string(&inbox.state.events[0])
            .unwrap()
            .contains("spoken_seq")
    );
}

#[test]
fn announcing_acknowledging_dismissing_and_snoozing() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    inbox.ingest(
        1,
        &e,
        mapped(EventKind::Done, Priority::Normal, "a", Some("a")),
    );
    inbox.ingest(
        2,
        &e,
        mapped(EventKind::Done, Priority::Normal, "b", Some("b")),
    );
    let plan = inbox.plan(10, true, 600, &eps).unwrap();
    inbox.mark_announced(&[plan.lead]);
    assert_eq!(inbox.reply_target().unwrap().id, plan.lead);
    assert!(inbox.snooze(plan.lead, 1000));
    assert_eq!(
        inbox.plan(10, true, 600, &eps).unwrap().lead,
        2,
        "snoozed one waits"
    );
    assert_eq!(
        inbox.plan(1000, true, 600, &eps).unwrap().lead,
        1,
        "and returns when due"
    );
    assert!(inbox.acknowledge(2));
    assert!(inbox.dismiss(1));
    assert!(inbox.plan(2000, true, 600, &eps).is_none());
    assert!(!inbox.acknowledge(99), "unknown ids are reported");
}

#[test]
fn what_are_my_agents_doing_groups_open_events_by_source() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    inbox.ingest(
        1,
        &e,
        mapped(
            EventKind::Done,
            Priority::Normal,
            "build finished",
            Some("a"),
        ),
    );
    inbox.ingest(
        2,
        &e,
        mapped(
            EventKind::NeedsInput,
            Priority::Urgent,
            "needs permission",
            Some("b"),
        ),
    );
    let mut other = mapped(EventKind::Done, Priority::Low, "backup ok", None);
    other.source = "Backup".into();
    inbox.ingest(3, &e, other);
    inbox.dismiss(3);
    let status = inbox.status_by_source();
    assert_eq!(status.len(), 1, "dismissed events do not count");
    assert_eq!(
        (
            status[0].waiting_on_you,
            status[0].finished,
            status[0].unheard
        ),
        (1, 1, 2)
    );
    assert_eq!(status[0].latest, "needs permission");
}

#[test]
fn prune_removes_old_handled_events_only() {
    let (mut inbox, eps) = setup();
    let e = ep(&eps);
    inbox.ingest(
        0,
        &e,
        mapped(EventKind::Done, Priority::Normal, "handled-old", Some("a")),
    );
    inbox.dismiss(1);
    inbox.ingest(
        0,
        &e,
        mapped(EventKind::Done, Priority::Normal, "unheard-old", Some("b")),
    );
    inbox.prune(100_000, 86_400);
    let titles: Vec<_> = inbox.events().iter().map(|x| x.title.as_str()).collect();
    assert_eq!(titles, vec!["unheard-old"]);
}

#[test]
fn the_inbox_survives_a_restart() {
    let dir = TempDir::new("inbox");
    let store = JsonStore::new(dir.0.join("inbox.json"), SCHEMA);
    let (mut inbox, eps) = setup();
    inbox.ingest(
        5,
        &ep(&eps),
        mapped(
            EventKind::NeedsInput,
            Priority::Urgent,
            "keep me",
            Some("s"),
        ),
    );
    inbox.save(&store).unwrap();
    let mut restored = Inbox::load(&store, InboxLimits::default()).unwrap();
    assert_eq!(restored.events().len(), 1);
    assert_eq!(
        restored.ingest(
            6,
            &ep(&eps),
            mapped(EventKind::Info, Priority::Low, "next", Some("t"))
        ),
        Ingest::Accepted(2),
        "ids keep counting"
    );
}
