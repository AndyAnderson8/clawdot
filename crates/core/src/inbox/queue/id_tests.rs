//! Events belong to an endpoint by its id (REQ-INB-013).

use super::*;
use crate::inbox::endpoint::QuietHours;
use crate::sched::testclock::StepClock;
use crate::store::testutil::TempDir;

fn mapped(title: &str, session: Option<&str>) -> Mapped {
    Mapped {
        source: "Claude Code".into(),
        kind: EventKind::Done,
        title: title.into(),
        text: format!("text of {title}"),
        status: None,
        priority: Priority::Normal,
        session_ref: session.map(str::to_owned),
        no_reply: false,
        ttl_secs: None,
    }
}

fn endpoints_with_cc() -> (Endpoints, Endpoint) {
    let mut eps = Endpoints::default();
    eps.create("cc", "generic", 0).unwrap();
    let cc = eps.get("cc").unwrap().clone();
    (eps, cc)
}

// REQ-INB-014: a later endpoint of the same name is not the one an earlier event came from: it is not merged
// into, and not dropped with it.
#[test]
fn events_follow_the_endpoint_id_not_its_name() {
    let (mut eps, old) = endpoints_with_cc();
    eps.revoke("cc").unwrap();
    eps.create("cc", "generic", 0).unwrap();
    let new = eps.get("cc").unwrap().clone();
    assert_ne!(old.id, new.id);

    let mut inbox = Inbox::new(InboxLimits::default());
    assert_eq!(
        inbox.ingest(1, &old, mapped("t", Some("s"))),
        Ingest::Accepted(1)
    );
    assert_eq!(
        inbox.ingest(2, &new, mapped("t", Some("s"))),
        Ingest::Accepted(2),
        "same name, session and kind, but not the same endpoint"
    );
    assert_eq!(
        inbox.ingest(3, &new, mapped("t", Some("s"))),
        Ingest::Coalesced(2)
    );

    inbox.set_origin_for(&new.id, "Kitchen");
    let origins: Vec<&str> = inbox.events().iter().map(Event::origin_name).collect();
    assert_eq!(origins, vec!["cc", "Kitchen"]);

    assert_eq!(inbox.drop_open_for(&old.id), 1);
    assert_eq!(inbox.events().len(), 1);
    assert_eq!(inbox.events()[0].endpoint_id, new.id);
}

// REQ-INB-040: a quiet window belongs to the endpoint with that id.
#[test]
fn an_endpoints_quiet_hours_are_found_by_id() {
    let (mut eps, old) = endpoints_with_cc();
    let mut inbox = Inbox::new(InboxLimits::default());
    inbox.ingest(1, &old, mapped("t", None));
    assert!(inbox.plan(1, true, 600, &eps).is_some());
    // Deleted, and another made under the name with quiet hours all day: the old event is not governed by
    // them, and the new endpoint's own events are.
    eps.revoke("cc").unwrap();
    eps.create("cc", "generic", 0).unwrap();
    eps.get_mut("cc").unwrap().filter.quiet_hours = Some(QuietHours {
        start: 0,
        end: 1_439,
    });
    let new = eps.get("cc").unwrap().clone();
    assert!(inbox.plan(1, true, 600, &eps).is_some());
    let mut inbox = Inbox::new(InboxLimits::default());
    inbox.ingest(1, &new, mapped("t", None));
    assert!(inbox.plan(1, true, 600, &eps).is_none());
}

// REQ-NFR-010: events saved before ids existed are tied to the live endpoint of their name, or dropped when
// there is none; events that have an id are not touched.
#[test]
fn events_saved_with_a_name_only_are_tied_to_the_live_endpoint_or_dropped() {
    let (eps, cc) = endpoints_with_cc();
    let mut inbox = Inbox::new(InboxLimits::default());
    for (t, title) in [(1, "a"), (2, "b"), (3, "c")] {
        inbox.ingest(t, &cc, mapped(title, None));
    }
    // Make them look like an old file: ids stripped; the second is from an endpoint that is gone.
    for e in &mut inbox.state.events {
        e.endpoint_id.clear();
    }
    inbox.state.events[1].endpoint = "gone".into();
    inbox.state.events[2].endpoint_id = "kept".into();

    assert_eq!(
        inbox.adopt_endpoint_ids(&eps),
        2,
        "one adopted, one dropped"
    );
    let held: Vec<(&str, &str)> = inbox
        .events()
        .iter()
        .map(|e| (e.title.as_str(), e.endpoint_id.as_str()))
        .collect();
    assert_eq!(held, vec![("a", cc.id.as_str()), ("c", "kept")]);
    assert_eq!(
        inbox.adopt_endpoint_ids(&eps),
        0,
        "a second pass changes nothing"
    );
}

// REQ-NFR-010: the id is written with the event, and an event without one (the old file) reads back empty.
#[test]
fn an_event_file_without_an_endpoint_id_loads_and_one_with_it_keeps_it() {
    let dir = TempDir::new("inbox-ids");
    let store = JsonStore::new(dir.0.join("inbox.json"), SCHEMA);
    let (_, cc) = endpoints_with_cc();
    let mut inbox = Inbox::new(InboxLimits::default());
    inbox.ingest(1, &cc, mapped("a", None));
    let clock = StepClock::at(1_800_000_000);
    inbox.save(&store, &clock).unwrap();
    let text = std::fs::read_to_string(store.path()).unwrap();
    let field = format!(r#""endpoint_id":"{}","#, cc.id);
    assert!(text.contains(&field));
    let loaded = Inbox::load(&store, InboxLimits::default(), &clock).unwrap();
    assert_eq!(loaded.events()[0].endpoint_id, cc.id);
    let old = text.replace(&field, "");
    assert!(!old.contains("endpoint_id"));
    std::fs::write(store.path(), old).unwrap();
    let loaded = Inbox::load(&store, InboxLimits::default(), &clock).unwrap();
    assert_eq!(loaded.events()[0].endpoint_id, "");
}
