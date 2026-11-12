use super::*;
use crate::app::{Adapters, LOCALHOST_PEER, Limits};
use crate::sched::{FixedOffset, days_from_civil};
use std::sync::atomic::{AtomicI64, Ordering};

const L: &Language = crate::lang::DEFAULT;

pub(super) struct TestClock(pub(super) AtomicI64);
impl Clock for TestClock {
    fn monotonic_secs(&self) -> i64 {
        self.now()
    }
    fn now(&self) -> Ts {
        self.0.load(Ordering::Relaxed)
    }
}

pub(super) fn utc(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> Ts {
    days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60
}

/// Saturday 2026-11-07 14:42 UTC, zone UTC.
pub(super) fn setup(with_core: bool) -> (Assistant, Arc<TestClock>, Option<Arc<Core>>) {
    let clock = Arc::new(TestClock(AtomicI64::new(utc(2026, 11, 7, 14, 42))));
    let core = with_core.then(|| {
        Arc::new(Core::in_memory(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
        ))
    });
    let a = Assistant::new(
        crate::lang::DEFAULT,
        clock.clone(),
        Box::new(FixedOffset(0)),
        Scheduler::new(16),
        None,
        core.clone(),
    );
    (a, clock, core)
}

fn say(a: &mut Assistant, u: &str) -> String {
    a.handle(u)
        .unwrap_or_else(|| panic!("fell through: {u}"))
        .say
}

#[test]
fn timers_end_to_end() {
    let (mut a, clock, _) = setup(false);
    assert_eq!(
        say(&mut a, "set a timer for ten minutes"),
        "Timer set for 10 minutes."
    );
    assert_eq!(
        say(&mut a, "set a pasta timer for 1 hour 30 minutes"),
        "Pasta timer set for 1 hour and 30 minutes."
    );
    assert_eq!(
        say(&mut a, "how much time is left on the pasta timer"),
        "1 hour and 30 minutes left on the pasta timer."
    );
    clock.0.fetch_add(600, Ordering::Relaxed);
    let fired = a.tick();
    assert_eq!(fired.len(), 1);
    assert_eq!(describe_fired(&fired[0]), "Your timer is done.");
    assert_eq!(say(&mut a, "cancel the pasta timer"), "Timer cancelled.");
    assert_eq!(
        say(&mut a, "cancel all timers"),
        "There are no timers to cancel."
    );
    assert_eq!(
        say(&mut a, "how much time is left"),
        "There are no timers running."
    );
}

#[test]
fn alarms_resolve_ambiguous_times_to_the_next_occurrence() {
    let (mut a, _, _) = setup(false);
    // 14:42 now: a bare "seven" means 19:00 today, "three" means 15:00 today, "two" means 02:00 tomorrow.
    assert_eq!(
        say(&mut a, "set an alarm for seven"),
        "Alarm set for 7 PM today."
    );
    assert_eq!(
        say(&mut a, "set an alarm for 3:30"),
        "Alarm set for 3:30 PM today."
    );
    assert_eq!(
        say(&mut a, "set an alarm for 6 am"),
        "Alarm set for 6 AM tomorrow."
    );
    assert_eq!(
        say(&mut a, "set a weekday alarm for 6:45 am"),
        "Alarm set for 6:45 AM on weekdays."
    );
    assert_eq!(say(&mut a, "cancel my alarms"), "4 alarms cancelled.");
    assert_eq!(say(&mut a, "cancel my alarm"), "You have no alarms set.");
}

#[test]
fn reminders() {
    let (mut a, clock, _) = setup(false);
    assert_eq!(
        say(&mut a, "remind me to call mum in ten minutes"),
        "Okay, I'll remind you to call mum in 10 minutes."
    );
    assert_eq!(
        say(&mut a, "remind me at 5 pm to start dinner"),
        "Okay, I'll remind you to start dinner at 5 PM today."
    );
    clock.0.fetch_add(601, Ordering::Relaxed);
    let fired = a.tick();
    assert_eq!(describe_fired(&fired[0]), "Reminder: call mum.");
}

#[test]
fn time_and_date_come_from_the_local_clock() {
    let (mut a, _, _) = setup(false);
    assert_eq!(say(&mut a, "what time is it"), "It's 2:42 PM.");
    assert_eq!(
        say(&mut a, "what's the date"),
        "Today is Saturday, November 7th."
    );
}

#[test]
fn listing_timers_is_answered_locally() {
    let (mut a, _, _) = setup(false);
    assert_eq!(say(&mut a, "list timers"), "There are no timers running.");
    say(&mut a, "set a pasta timer for 10 minutes");
    say(&mut a, "set a timer for 5 minutes");
    assert_eq!(
        say(&mut a, "what timers do I have"),
        "5 minutes left on your timer, 10 minutes left on the pasta timer."
    );
}

#[test]
fn an_unset_clock_is_admitted_not_read_out_as_the_time() {
    // REQ-TMR-009: a device with no battery-backed clock and no network boots at 2010-01-01.
    let (mut a, clock, _) = setup(false);
    clock.0.store(utc(2010, 1, 1, 0, 3), Ordering::Relaxed);
    for u in [
        "what time is it",
        "what's the date",
        "set an alarm for 7 am",
        "set an alarm for 7 am every day",
        "remind me in 5 minutes to stretch",
        "remind me to call mum at 5 pm",
    ] {
        assert_eq!(say(&mut a, u), CLOCK_UNSET, "{u}");
    }
    assert!(a.scheduled().is_empty(), "no alarm or reminder may be set");
    // Commands that do not depend on the time still work.
    assert_eq!(
        say(&mut a, "cancel the timer"),
        "There are no timers to cancel."
    );
    // A countdown timer needs no calendar: it is set, listed and answered on the monotonic clock.
    assert_eq!(
        say(&mut a, "set a timer for ten minutes"),
        "Timer set for 10 minutes."
    );
    assert_eq!(
        say(&mut a, "how much time is left"),
        "10 minutes left on your timer."
    );
    assert_eq!(a.scheduled().len(), 1);
    assert_eq!(say(&mut a, "cancel the timer"), "Timer cancelled.");
    // Once the clock is set, the same questions are answered.
    clock.0.store(utc(2026, 11, 7, 14, 42), Ordering::Relaxed);
    assert_eq!(say(&mut a, "what time is it"), "It's 2:42 PM.");
}

#[test]
fn volume_media_and_stop_produce_effects() {
    let (mut a, _, _) = setup(false);
    assert_eq!(
        a.handle("volume up").unwrap().effects,
        vec![Effect::VolumeUp]
    );
    assert_eq!(
        a.handle("set the volume to 40 percent").unwrap().effects,
        vec![Effect::VolumeSet(40)]
    );
    assert!(
        a.handle("set the volume to 400").is_none(),
        "out of range falls through"
    );
    assert_eq!(
        a.handle("pause the music").unwrap().effects,
        vec![Effect::Media(MediaCmd::Pause)]
    );
    assert_eq!(
        a.handle("stop").unwrap().effects,
        vec![Effect::StopPlayback]
    );
}

#[test]
fn everything_else_falls_through() {
    let (mut a, _, _) = setup(true);
    for u in ["what's the capital of France", "tell me a joke", "thanks"] {
        assert!(a.handle(u).is_none(), "{u}");
    }
}

#[test]
fn limits_are_spoken_not_silently_ignored() {
    let clock = Arc::new(TestClock(AtomicI64::new(utc(2026, 11, 7, 14, 42))));
    let mut a = Assistant::new(
        crate::lang::DEFAULT,
        clock,
        Box::new(FixedOffset(0)),
        Scheduler::new(1),
        None,
        None,
    );
    say(&mut a, "set a timer for 1 minute");
    assert_eq!(
        say(&mut a, "set a timer for 2 minutes"),
        "Sorry, too many timers, alarms, and reminders."
    );
    assert!(say(&mut a, "set a timer for 30 days").starts_with("Sorry"));
}

const STOP_PAYLOAD: &str = r#"{"session_id":"s1","cwd":"/home/a/clawdot","hook_event_name":"Stop","last_assistant_message":"Added the timer tool. Tests pass."}"#;

#[test]
fn the_agent_loop_by_voice() {
    let (mut a, _, core) = setup(true);
    let core = core.unwrap();
    let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();

    // Nothing open yet: inbox phrases fall through (or answer plainly).
    assert!(a.handle("thanks I'll handle it").is_none());
    assert!(a.handle("keep going").is_none());
    assert_eq!(
        say(&mut a, "what are my agents up to"),
        "Nothing from your agents right now."
    );

    core.ingest_hook(
        "claude-code",
        &token,
        LOCALHOST_PEER,
        STOP_PAYLOAD.as_bytes(),
    )
    .unwrap();
    assert_eq!(say(&mut a, "any updates"), "claude-code: 1 finished.");

    let id = core.next_announcement(true, 0).unwrap().event.id;
    core.mark_announced(&[id]);
    assert_eq!(
        say(&mut a, "give me the details"),
        "Added the timer tool. Tests pass."
    );
    assert_eq!(
        say(&mut a, "tell it to also add unit tests"),
        "Okay, sending that to claude-code."
    );
    let pending = core
        .bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
        .unwrap();
    assert_eq!(
        (pending[0].text.as_str(), pending[0].session_ref.as_deref()),
        ("also add unit tests", Some("s1"))
    );
}

#[test]
fn thanks_ill_handle_it_tells_the_agent_not_to_wait() {
    let (mut a, _, core) = setup(true);
    let core = core.unwrap();
    let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();
    core.ingest_hook(
        "claude-code",
        &token,
        LOCALHOST_PEER,
        STOP_PAYLOAD.as_bytes(),
    )
    .unwrap();
    let id = core.next_announcement(true, 0).unwrap().event.id;
    core.mark_announced(&[id]);
    assert_eq!(
        say(&mut a, "thanks I'll handle it"),
        "Okay. I'll let claude-code know you've got it."
    );
    assert_eq!(
        core.bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
            .unwrap()[0]
            .kind,
        ReplyKind::Ack
    );
}

#[test]
fn dismiss_and_snooze_apply_to_the_open_event() {
    let (mut a, _, core) = setup(true);
    let core = core.unwrap();
    let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();
    core.ingest_hook(
        "claude-code",
        &token,
        LOCALHOST_PEER,
        STOP_PAYLOAD.as_bytes(),
    )
    .unwrap();
    let id = core.next_announcement(true, 0).unwrap().event.id;
    core.mark_announced(&[id]);
    assert_eq!(
        say(&mut a, "snooze for twenty minutes"),
        "Okay, I'll bring it up again in 20 minutes."
    );
    assert!(core.next_announcement(true, 0).is_none());
    let out = a.handle("dismiss").unwrap();
    assert_eq!(
        out.effects,
        vec![Effect::StopPlayback],
        "nothing open, so it acts like stop"
    );
}

#[test]
fn formatting_helpers() {
    assert_eq!(format_duration(L, 45), "45 seconds");
    assert_eq!(format_duration(L, 60), "1 minute");
    assert_eq!(format_duration(L, 3661), "1 hour, 1 minute and 1 second");
    assert_eq!(format_clock(L, 0, 5), "12:05 AM");
    assert_eq!(format_clock(L, 12, 0), "12 PM");
    assert_eq!(format_clock(L, 23, 30), "11:30 PM");
    assert_eq!(
        (
            ordinal(L, 1),
            ordinal(L, 2),
            ordinal(L, 3),
            ordinal(L, 4),
            ordinal(L, 11),
            ordinal(L, 12),
            ordinal(L, 21),
            ordinal(L, 22)
        ),
        (
            "1st".into(),
            "2nd".into(),
            "3rd".into(),
            "4th".into(),
            "11th".into(),
            "12th".into(),
            "21st".into(),
            "22nd".into()
        )
    );
}

#[test]
fn scheduled_items_persist_through_the_store() {
    let dir = crate::store::testutil::TempDir::new("assistant");
    let store = JsonStore::new(dir.0.join("sched.json"), crate::sched::SCHEMA);
    let clock = Arc::new(TestClock(AtomicI64::new(utc(2026, 11, 7, 14, 42))));
    {
        let mut a = Assistant::new(
            crate::lang::DEFAULT,
            clock.clone(),
            Box::new(FixedOffset(0)),
            Scheduler::new(16),
            Some(store.clone()),
            None,
        );
        say(&mut a, "set a timer for ten minutes");
    }
    let sched = Scheduler::load(&store, 16, &*clock).unwrap();
    assert_eq!(sched.list(&*clock).len(), 1);
}

// ----- music (REQ-MUS-001, 002) -----

fn radio() -> Assistant {
    let (a, ..) = setup(false);
    a.with_stations(vec![
        Station {
            name: "jazz".into(),
            url: "http://r.example/jazz".into(),
        },
        Station {
            name: "bbc world service".into(),
            url: "http://r.example/bbc".into(),
        },
    ])
}

fn played(o: &Outcome) -> Option<&str> {
    o.effects.iter().find_map(|e| match e {
        Effect::PlayStream { title, .. } => Some(title.as_str()),
        _ => None,
    })
}

#[test]
fn play_by_name_starts_the_stream() {
    let mut a = radio();
    let o = a.handle("play jazz").unwrap();
    assert_eq!(o.say, "Playing jazz.");
    assert_eq!(played(&o), Some("jazz"));
    let o = a.handle("play the bbc world service").unwrap();
    assert_eq!(played(&o), Some("bbc world service"));
    let o = a.handle("put on some jazz music").unwrap();
    assert_eq!(played(&o), Some("jazz"));
}

#[test]
fn transport_commands_follow_the_player() {
    let mut a = radio();
    assert!(
        a.handle("pause")
            .unwrap()
            .say
            .contains("Nothing is playing")
    );
    a.handle("play jazz").unwrap();
    assert_eq!(
        a.handle("pause the music").unwrap().effects,
        vec![Effect::Media(MediaCmd::Pause)]
    );
    assert_eq!(
        a.handle("resume").unwrap().effects,
        vec![Effect::Media(MediaCmd::Resume)]
    );
    assert_eq!(
        played(&a.handle("next station").unwrap()),
        Some("bbc world service")
    );
    assert_eq!(played(&a.handle("go back").unwrap()), Some("jazz"));
    // "stop" halts playback, and the selection survives for "resume".
    assert_eq!(
        a.handle("stop").unwrap().effects,
        vec![Effect::StopPlayback]
    );
    assert_eq!(
        a.handle("play the music").unwrap().effects,
        vec![Effect::Media(MediaCmd::Resume)]
    );
}

#[test]
fn an_unknown_station_is_answered_locally_but_without_stations_play_goes_to_the_model() {
    let mut a = radio();
    let o = a.handle("play polka").unwrap();
    assert!(o.say.contains("jazz") && o.effects.is_empty(), "{o:?}");
    let (mut bare, ..) = setup(false);
    assert!(
        bare.handle("play a game with me").is_none(),
        "no stations: the model decides"
    );
    // Without stations the old controls still produce their effects.
    assert_eq!(
        bare.handle("pause").unwrap().effects,
        vec![Effect::Media(MediaCmd::Pause)]
    );
}

// ----- a stepping wall clock (REQ-TMR-002, REQ-TMR-003, REQ-TMR-006) -----

fn stepping(
    dir: Option<&crate::store::testutil::TempDir>,
    wall: Ts,
    mono: i64,
) -> (Assistant, Arc<crate::sched::testclock::StepClock>) {
    let clock = Arc::new(crate::sched::testclock::StepClock::with_mono(wall, mono));
    let store = dir.map(|d| JsonStore::new(d.0.join("sched.json"), crate::sched::SCHEMA));
    let sched = store.as_ref().map_or_else(
        || Scheduler::new(16),
        |s| Scheduler::load(s, 16, &*clock).unwrap(),
    );
    let a = Assistant::new(
        crate::lang::DEFAULT,
        clock.clone(),
        Box::new(FixedOffset(0)),
        sched,
        store,
        None,
    );
    (a, clock)
}

// The failure in numbers: the clock is 3 h behind when the user says "ten minutes", and network time
// arrives 20 s later.
#[test]
fn a_spoken_timer_set_while_the_clock_is_behind_rings_on_time_after_the_clock_is_corrected() {
    let (mut a, clock) = stepping(None, utc(2026, 11, 7, 11, 42), 100);
    assert_eq!(
        say(&mut a, "set a timer for ten minutes"),
        "Timer set for 10 minutes."
    );
    clock.advance(20);
    clock.step_wall(3 * 3600);
    assert!(
        a.tick().is_empty(),
        "not announced as missed seconds after it was set"
    );
    assert_eq!(a.scheduled()[0].2, 580);
    assert_eq!(
        say(&mut a, "how much time is left"),
        "9 minutes and 40 seconds left on your timer."
    );
    clock.advance(579);
    assert!(a.tick().is_empty());
    clock.advance(1);
    let fired = a.tick();
    assert!(
        fired.len() == 1 && !fired[0].missed && fired[0].late_secs == 0,
        "{fired:?}"
    );
}

#[test]
fn a_backward_step_does_not_make_a_timer_ring_late() {
    let (mut a, clock) = stepping(None, utc(2026, 11, 7, 14, 42), 100);
    say(&mut a, "set a timer for ten minutes");
    clock.advance(300);
    clock.step_wall(-3 * 3600);
    clock.advance(300);
    assert_eq!(a.tick().len(), 1);
}

#[test]
fn a_reminder_after_a_delay_is_a_duration_but_a_reminder_at_a_time_is_a_calendar_point() {
    let (mut a, clock) = stepping(None, utc(2026, 11, 7, 14, 42), 100);
    say(&mut a, "remind me in 5 minutes to stretch");
    say(&mut a, "remind me to call mum at 5 pm");
    clock.advance(300);
    clock.step_wall(-1800); // 14:17
    let fired = a.tick();
    assert_eq!(fired.len(), 1, "the delayed one rang on time: {fired:?}");
    assert!(fired[0].label.contains("stretch"));
    // The wall clock is corrected to 17:00: the calendar reminder rings then, not before.
    clock.set_wall(utc(2026, 11, 7, 16, 59));
    assert!(a.tick().is_empty());
    clock.advance(60);
    assert_eq!(a.tick().len(), 1);
}

#[test]
fn a_restart_takes_the_time_off_a_saved_timer_and_the_portal_listing_follows_the_clock() {
    let dir = crate::store::testutil::TempDir::new("assistant-step");
    let (mut a, clock) = stepping(Some(&dir), utc(2026, 11, 7, 14, 42), 100);
    say(&mut a, "set a timer for ten minutes");
    clock.advance(60);
    // A different timer set later makes the file be written again with the first one's figure refreshed.
    say(&mut a, "set a timer for an hour");
    drop(a);
    // The speaker was off for 90 s; the new process has a new monotonic origin.
    let (a, after) = stepping(Some(&dir), clock.now() + 90, 5);
    let left: Vec<i64> = a.scheduled().iter().map(|t| t.2).collect();
    assert_eq!(left, [600 - 60 - 90, 3600 - 90]);
    // What the portal shows is derived at read time: after a step it is the corrected wall time.
    after.step_wall(3 * 3600);
    let mut a = a;
    let list = a.admin_sched(crate::app::SchedOp::List).unwrap();
    assert_eq!(list["items"][0]["seconds_left"], 450);
    assert_eq!(list["items"][0]["fire_at"], after.now() + 450);
}
