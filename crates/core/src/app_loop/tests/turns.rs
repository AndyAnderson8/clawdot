//! What the loop does with a request and its reply (`App::dispatch`, `App::deliver_confirmed`).

use super::*;
use crate::app_loop::turns::NO_LLM;

#[tokio::test]
async fn the_fast_path_answers_without_a_model_and_other_requests_say_one_is_needed() {
    let dir = TempDir::new("dispatch");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let ask = |line: &str| Request {
        speaker: "console".into(),
        line: line.to_owned(),
        ctl: TurnCtl::default(),
        origin: Origin::Console,
    };
    let (_, reply) = app.dispatch(ask("set a timer for 5 minutes")).unwrap();
    assert!(
        !reply.say.is_empty() && reply.say != NO_LLM,
        "{}",
        reply.say
    );
    let (_, reply) = app
        .dispatch(ask("tell me about owls and their habits"))
        .unwrap();
    assert_eq!(reply.say, NO_LLM);
}

fn fast_request(line: &str, origin: Origin) -> Request {
    Request {
        speaker: "console".into(),
        line: line.to_owned(),
        ctl: TurnCtl::default(),
        origin,
    }
}

// REQ-TMR-003: a timer the user was told is set survives a crash right after the confirmation. The reply is
// held back until the write lands, and the loop is not held with it.
#[tokio::test(flavor = "current_thread")]
async fn the_confirmation_of_a_timer_is_not_delivered_before_it_is_on_disk() {
    let dir = TempDir::new("timer-durable");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let writer = app.core.writer().unwrap();
    writer.pause();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let (origin, reply) = app
        .dispatch(fast_request("set a timer for 5 minutes", Origin::Ask(tx)))
        .unwrap();
    assert!(reply.say.starts_with("Timer set"), "{}", reply.say);
    app.deliver_confirmed(origin, reply);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(rx.try_recv().is_err(), "spoken before it was saved");
    assert_eq!(timers_on_disk(&dir), 0);
    writer.resume();
    let said = rx.await.unwrap();
    assert!(said.say.starts_with("Timer set"));
    // The confirmation is out and nothing has been flushed by hand: it is already on disk.
    assert_eq!(timers_on_disk(&dir), 1);
}

// REQ-TMR-003: a timer whose save failed is still set (it will ring), and the confirmation says so in one short
// sentence instead of claiming it is safe. The next change that does save is confirmed plainly again, and a reply
// that changed nothing never repeats the bad news.
#[tokio::test(flavor = "current_thread")]
async fn a_timer_that_could_not_be_saved_is_confirmed_honestly() {
    let dir = TempDir::new("timer-unsaved");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    // A directory where the temporary file would be written makes every save fail.
    let blocker = crate::store::temp_path(&dir.0.join("timers.json"));
    std::fs::create_dir_all(&blocker).unwrap();
    async fn said(app: &mut App, line: &str) -> String {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (origin, reply) = app.dispatch(fast_request(line, Origin::Ask(tx))).unwrap();
        app.deliver_confirmed(origin, reply);
        rx.await.unwrap().say
    }
    let first = said(&mut app, "set a timer for 10 minutes").await;
    assert_eq!(
        first,
        "Timer set for 10 minutes, but I could not save it, so it will be lost if I restart.",
        "{first}"
    );
    let unrelated = said(&mut app, "what time is it").await;
    assert!(!unrelated.contains("save"), "{unrelated}");
    assert_eq!(app.brain.assistant.scheduled().len(), 1, "it is still set");
    std::fs::remove_dir(&blocker).unwrap();
    let later = said(&mut app, "set a timer for 5 minutes").await;
    assert!(
        later.starts_with("Timer set") && !later.contains("save"),
        "{later}"
    );
    assert_eq!(timers_on_disk(&dir), 2, "the later save carried both");
}

// A reply that announces nothing new is not held back by a write that is still queued.
#[tokio::test(flavor = "current_thread")]
async fn a_reply_with_no_pending_timer_change_is_delivered_at_once() {
    let dir = TempDir::new("timer-no-wait");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let writer = app.core.writer().unwrap();
    writer.pause();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let (origin, reply) = app
        .dispatch(fast_request("what time is it", Origin::Ask(tx)))
        .unwrap();
    app.deliver_confirmed(origin, reply);
    assert!(rx.try_recv().is_ok());
    writer.resume();
}

// Rapid changes coalesce, and the file never goes back to an older state.
#[tokio::test(flavor = "current_thread")]
async fn rapid_timer_changes_leave_the_newest_state_on_disk() {
    let dir = TempDir::new("timer-rapid");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    for n in 1..=5 {
        let line = format!("set a timer for {n} minutes");
        app.dispatch(fast_request(&line, Origin::Console)).unwrap();
    }
    app.dispatch(fast_request("cancel the timer", Origin::Console))
        .unwrap();
    for n in 6..=7 {
        let line = format!("set a timer for {n} minutes");
        app.dispatch(fast_request(&line, Origin::Console)).unwrap();
    }
    let expected = app.brain.assistant.scheduled().len();
    app.core.flush_blocking();
    assert_eq!(timers_on_disk(&dir), expected);
    assert_eq!(
        expected, 2,
        "the cancel emptied the list before the last two"
    );
}
