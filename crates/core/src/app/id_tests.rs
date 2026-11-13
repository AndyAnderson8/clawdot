//! Endpoint ids (REQ-INB-013): what an endpoint holds is keyed by its id, not by its name.

use super::announce::{AuditAs, Caller};
use super::ask::AskError;
use super::tests::TestClock;
use super::*;
use crate::store::testutil::TempDir;
use std::sync::atomic::AtomicI64;

fn core() -> Core {
    let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
    Core::in_memory(clock, Adapters::builtin(), Limits::default())
}

/// Remove an endpoint the way a path would that forgot to clean up after it: from the list only. Nothing keyed
/// by the endpoint is dropped. Since everything is keyed by the id, a new endpoint of that name must still reach
/// none of it.
fn revoke_without_cleanup(c: &Core, name: &str) {
    c.lock().endpoints.revoke(name).unwrap();
}

fn hook(c: &Core, name: &str, token: &str, n: u32) -> Result<HookOutcome, HookError> {
    c.ingest_hook(
        name,
        token,
        LOCALHOST_PEER,
        format!(r#"{{"title":"t{n}","session_ref":"s{n}"}}"#).as_bytes(),
    )
}

fn replies_waiting(c: &Core) -> usize {
    c.lock().replies.len()
}

fn question() -> Announcement {
    Announcement::from_json(&serde_json::json!({"text": "Shall I?", "reply": true})).unwrap()
}

const AS: AuditAs<'static> = AuditAs {
    trigger: "t",
    tool: "ask_user",
};

#[test]
fn a_recreated_endpoint_has_a_new_id_and_the_name_is_free_again() {
    let c = core();
    c.create_endpoint("agent", "generic").unwrap();
    let old = c.endpoints()[0].id.clone();
    assert_eq!(old.len(), 32);
    assert!(
        old.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    c.revoke_endpoint("agent").unwrap();
    c.create_endpoint("agent", "generic").unwrap();
    assert_ne!(c.endpoints()[0].id, old);
}

#[test]
fn a_new_endpoint_cannot_reach_the_old_ones_rate_limit_even_if_cleanup_is_forgotten() {
    let c = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    let mut n = 0;
    while hook(&c, "agent", &old, n).is_ok() {
        n += 1;
        assert!(n < 1000, "never throttled");
    }
    revoke_without_cleanup(&c, "agent");
    let (_, new) = c.create_endpoint("agent", "generic").unwrap();
    assert!(
        hook(&c, "agent", &new, 0).is_ok(),
        "the new endpoint was throttled by the old one's use"
    );
}

#[test]
fn announcements_are_limited_per_endpoint_id_too() {
    let c = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    let a = Announcement::from_json(&serde_json::json!({"text": "Hello there."})).unwrap();
    let mut n = 0;
    while c.announce(Caller::Token(&old, LOCALHOST_PEER), &a).is_ok() {
        n += 1;
        assert!(n < 1000, "never throttled");
    }
    revoke_without_cleanup(&c, "agent");
    let (_, new) = c.create_endpoint("agent", "generic").unwrap();
    assert!(c.announce(Caller::Token(&new, LOCALHOST_PEER), &a).is_ok());
}

#[test]
fn a_new_endpoint_cannot_reach_the_old_ones_replies_even_if_cleanup_is_forgotten() {
    let c = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    hook(&c, "agent", &old, 1).unwrap();
    let first = c.next_announcement(true, 0).unwrap().event.id;
    c.mark_announced(&[first]);
    c.reply(ReplyKind::Instruct, "private to the old agent")
        .unwrap();
    assert_eq!(replies_waiting(&c), 1);

    revoke_without_cleanup(&c, "agent");
    let (_, new) = c.create_endpoint("agent", "generic").unwrap();

    assert!(
        c.bridge_pending("agent", &new, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty(),
        "the new endpoint got the old one's replies"
    );
    assert!(
        !c.bridge_ack("agent", &new, LOCALHOST_PEER, 1).unwrap(),
        "the new bridge acknowledged the old reply"
    );
    assert_eq!(replies_waiting(&c), 1);

    // An answer to the new endpoint's own event reaches its bridge, and the bridge sees the name on the wire.
    hook(&c, "agent", &new, 2).unwrap();
    let second = c
        .events()
        .iter()
        .map(|e| e.id)
        .find(|&i| i != first)
        .unwrap();
    c.mark_announced(&[second]);
    c.reply_to(second, ReplyKind::Instruct, "for the new agent")
        .unwrap();
    let got = c.bridge_pending("agent", &new, LOCALHOST_PEER, 0).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].text, "for the new agent");
    assert_eq!(got[0].endpoint, "agent");
}

#[test]
fn a_new_endpoints_events_are_not_merged_into_the_old_ones_even_if_cleanup_is_forgotten() {
    let c = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    hook(&c, "agent", &old, 1).unwrap();
    revoke_without_cleanup(&c, "agent");
    let (_, new) = c.create_endpoint("agent", "generic").unwrap();
    // Same session and kind inside the coalesce window: with the name as the key this would be merged into the
    // old endpoint's unspoken event.
    assert!(matches!(
        hook(&c, "agent", &new, 1),
        Ok(HookOutcome::Accepted(_))
    ));
    assert_eq!(c.events().len(), 2);
}

#[test]
fn a_question_being_asked_does_not_block_a_new_endpoint_of_the_same_name() {
    let c = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    let old_ticket = c.ask_open(&old, LOCALHOST_PEER, &question(), AS).unwrap();
    // One question per endpoint: the same endpoint is busy.
    assert!(matches!(
        c.ask_open(&old, LOCALHOST_PEER, &question(), AS),
        Err(AskError::Busy)
    ));

    revoke_without_cleanup(&c, "agent");
    let (_, new) = c.create_endpoint("agent", "generic").unwrap();
    let new_ticket = c.ask_open(&new, LOCALHOST_PEER, &question(), AS);
    assert!(new_ticket.is_ok(), "the old question made the new one busy");
    drop((old_ticket, new_ticket));
}

#[test]
fn deleting_an_endpoint_ends_the_question_it_was_asking() {
    let c = core();
    let (_, old) = c.create_endpoint("agent", "generic").unwrap();
    let t = c.ask_open(&old, LOCALHOST_PEER, &question(), AS).unwrap();
    let event = c.events()[0].id;
    assert!(c.ask_pending(event));
    c.revoke_endpoint("agent").unwrap();
    assert!(
        !c.ask_pending(event),
        "the held question outlived its endpoint"
    );
    drop(t);
}

/// Rewrite the three state files the way the version before endpoint ids wrote them: no `id` on an endpoint, no
/// `endpoint_id` on an event or a reply.
fn make_old_format(dir: &TempDir) {
    let edit = |file: &str, list: &str, key: Option<&str>, drop_name: Option<&str>| {
        let path = dir.0.join(file);
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let items = v["data"][list].as_array_mut().unwrap();
        if let Some(name) = drop_name {
            items.retain(|e| e["name"] != name);
        }
        for item in items {
            if let Some(key) = key {
                assert!(
                    item.as_object_mut().unwrap().remove(key).is_some(),
                    "{file}"
                );
            }
        }
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    };
    // The endpoint "gone" was deleted before the upgrade, leaving its events and replies behind.
    edit("endpoints.json", "list", Some("id"), Some("gone"));
    edit("inbox.json", "events", Some("endpoint_id"), None);
    edit("replies.json", "pending", Some("endpoint_id"), None);
}

// REQ-NFR-010: files saved before endpoints had ids load. Each endpoint gets an id, which is written at once
// (a later restart must not change it); events and replies saved with a name only are tied to the live endpoint
// of that name, and dropped when there is none.
#[test]
fn files_saved_before_endpoint_ids_load_and_keep_what_belongs_to_a_live_endpoint() {
    let dir = TempDir::new("core-old-ids");
    let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
    let open = || {
        Core::open(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&dir.0),
        )
        .unwrap()
    };
    let kept_token;
    {
        let c = open();
        kept_token = c.create_endpoint("kept", "generic").unwrap().1;
        let gone_token = c.create_endpoint("gone", "generic").unwrap().1;
        for (name, token) in [("kept", &kept_token), ("gone", &gone_token)] {
            hook(&c, name, token, 1).unwrap();
            let id = c
                .events()
                .iter()
                .rev()
                .find(|e| e.endpoint == name)
                .unwrap()
                .id;
            c.mark_announced(&[id]);
            c.reply_to(id, ReplyKind::Instruct, &format!("for {name}"))
                .unwrap();
        }
        assert_eq!((c.events().len(), replies_waiting(&c)), (2, 2));
        c.flush_blocking();
    }
    make_old_format(&dir);

    let c = open();
    assert_eq!(c.endpoints().len(), 1);
    let id = c.endpoints()[0].id.clone();
    assert_eq!(id.len(), 32, "an endpoint saved without an id gets one");
    assert!(
        std::fs::read_to_string(dir.0.join("endpoints.json"))
            .unwrap()
            .contains(&id),
        "the new id is on disk before anything is keyed by it"
    );
    let events = c.events();
    assert_eq!(
        events.len(),
        1,
        "the event of the deleted endpoint is dropped"
    );
    assert_eq!(
        (events[0].endpoint.as_str(), events[0].endpoint_id.as_str()),
        ("kept", id.as_str())
    );
    let got = c
        .bridge_pending("kept", &kept_token, LOCALHOST_PEER, 0)
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].text, "for kept");
    assert_eq!(
        replies_waiting(&c),
        1,
        "the reply of the deleted endpoint is dropped"
    );
    c.flush_blocking();
    drop(c);

    // A second start finds the ids on disk: nothing changes and nothing is orphaned.
    let c = open();
    assert_eq!(c.endpoints()[0].id, id);
    assert_eq!(c.events().len(), 1);
    assert_eq!(
        c.bridge_pending("kept", &kept_token, LOCALHOST_PEER, 0)
            .unwrap()
            .len(),
        1
    );
}
