use super::testclock::StepClock;
use super::*;
use crate::store::testutil::TempDir;

/// UTC instant for a calendar time.
fn utc(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> Ts {
    days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60
}

const T0: Ts = 1_794_225_600; // 2026-11-09 12:00 UTC

fn utc0() -> FixedOffset {
    FixedOffset(0)
}

#[test]
fn calendar_helpers_agree_with_known_dates() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    assert_eq!(weekday(0), 3, "1970-01-01 was a Thursday");
    assert_eq!(
        weekday(days_from_civil(2026, 11, 7)),
        5,
        "2026-11-07 is a Saturday"
    );
    for day in (-800_000..800_000).step_by(997) {
        let (y, m, d) = civil_from_days(day);
        assert_eq!(days_from_civil(y, m, d), day);
    }
}

#[test]
fn a_timer_fires_at_its_deadline_and_not_before() {
    let mut s = Scheduler::new(8);
    let t0 = utc(2026, 11, 7, 12, 0);
    let clock = StepClock::at(t0);
    let id = s.set_timer(&clock, 600, "pasta").unwrap();
    clock.advance(599);
    assert!(s.take_due(&clock, &utc0()).is_empty());
    assert_eq!(s.remaining(id, &clock).unwrap(), 1);
    clock.advance(1);
    let due = s.take_due(&clock, &utc0());
    assert_eq!(due.len(), 1);
    assert_eq!(
        (due[0].kind, due[0].label.as_str(), due[0].late_secs),
        (Kind::Timer, "pasta", 0)
    );
    assert!(s.list(&clock).is_empty(), "fired one-shots are removed");
}

// REQ-TMR-002
#[test]
fn a_ten_minute_timer_rings_after_600_monotonic_seconds_whatever_the_wall_clock_does() {
    for step in [
        3 * 3600,
        -3 * 3600,
        240,
        -240,
        MAX_TIMER_SECS,
        -MAX_TIMER_SECS,
    ] {
        let mut s = Scheduler::new(8);
        let clock = StepClock::at(T0);
        s.set_timer(&clock, 600, "pasta").unwrap();
        clock.advance(20);
        clock.step_wall(step);
        clock.advance(579);
        assert!(
            s.take_due(&clock, &utc0()).is_empty(),
            "rang early after a step of {step} s"
        );
        clock.advance(1);
        let due = s.take_due(&clock, &utc0());
        assert_eq!(due.len(), 1, "did not ring after a step of {step} s");
        assert!(
            !due[0].missed && due[0].late_secs == 0,
            "a step of {step} s made it late: {:?}",
            due[0]
        );
    }
}

// The failure the write-up describes in numbers: set 20 s after boot with the clock 3 h behind, then
// network time steps it forward 10,800 s.
#[test]
fn a_timer_set_with_the_clock_behind_is_not_announced_as_missed_when_it_is_corrected() {
    let mut s = Scheduler::new(8);
    let clock = StepClock::at(T0 - 10_800 + 20);
    s.set_timer(&clock, 600, "tea").unwrap();
    clock.advance(20);
    clock.step_wall(10_800);
    clock.advance(1);
    assert!(
        s.take_due(&clock, &utc0()).is_empty(),
        "still 579 s to go on the timer's own clock"
    );
    clock.advance(579);
    let due = s.take_due(&clock, &utc0());
    assert!(due.len() == 1 && !due[0].missed);
}

#[test]
fn what_a_person_is_shown_follows_the_countdown_not_the_old_wall_deadline() {
    let mut s = Scheduler::new(8);
    let clock = StepClock::at(T0);
    let id = s.set_timer(&clock, 600, "").unwrap();
    clock.advance(100);
    clock.step_wall(3 * 3600);
    assert_eq!(s.remaining(id, &clock).unwrap(), 500, "no jump on a step");
    let item = s.list(&clock)[0].clone();
    assert_eq!(item.secs_left(&clock), 500);
    assert_eq!(
        item.due_wall(&clock),
        clock.now() + 500,
        "the time it rings is worked out from the wall clock as it is now"
    );
    assert!(item.is_countdown());
}

#[test]
fn the_list_is_ordered_by_when_things_come_due_after_a_step() {
    let mut s = Scheduler::new(8);
    let zone = utc0();
    let clock = StepClock::at(utc(2026, 11, 9, 6, 0));
    s.set_alarm(clock.now(), &zone, 7, 0, None, "alarm")
        .unwrap();
    s.set_timer(&clock, 600, "timer").unwrap();
    clock.step_wall(3600);
    let labels: Vec<_> = s.list(&clock).iter().map(|i| i.label.clone()).collect();
    assert_eq!(
        labels,
        ["alarm", "timer"],
        "the alarm is due at 07:00 sharp"
    );
    clock.step_wall(-2 * 3600);
    let labels: Vec<_> = s.list(&clock).iter().map(|i| i.label.clone()).collect();
    assert_eq!(labels, ["timer", "alarm"]);
}

#[test]
fn durations_are_validated() {
    let mut s = Scheduler::new(8);
    let clock = StepClock::at(1_800_000_000);
    for bad in [0, -5, MAX_TIMER_SECS + 1] {
        assert_eq!(s.set_timer(&clock, bad, ""), Err(SchedError::InvalidTime));
    }
    assert!(s.set_timer(&clock, MAX_TIMER_SECS, "").is_ok());
}

#[test]
fn the_item_limit_is_enforced() {
    let mut s = Scheduler::new(2);
    let clock = StepClock::at(1_800_000_000);
    s.set_timer(&clock, 10, "a").unwrap();
    s.set_timer(&clock, 10, "b").unwrap();
    assert_eq!(s.set_timer(&clock, 10, "c"), Err(SchedError::TooMany));
}

#[test]
fn a_one_off_alarm_rings_today_if_the_time_is_ahead_otherwise_tomorrow() {
    let mut s = Scheduler::new(8);
    let zone = utc0();
    let clock = StepClock::at(utc(2026, 11, 7, 6, 0)); // Saturday 06:00
    s.set_alarm(clock.now(), &zone, 7, 30, None, "wake")
        .unwrap();
    assert_eq!(s.list(&clock)[0].fire_at, utc(2026, 11, 7, 7, 30));
    let later = utc(2026, 11, 7, 8, 0);
    s.set_alarm(later, &zone, 7, 30, None, "wake2").unwrap();
    assert_eq!(s.list(&clock)[1].fire_at, utc(2026, 11, 8, 7, 30));
}

#[test]
fn weekday_alarms_skip_the_weekend_and_reschedule_after_firing() {
    let mut s = Scheduler::new(8);
    let zone = utc0();
    let friday_evening = utc(2026, 11, 6, 20, 0);
    s.set_alarm(friday_evening, &zone, 7, 0, Some(days::WEEKDAYS), "work")
        .unwrap();
    let clock = StepClock::at(utc(2026, 11, 9, 7, 0));
    assert_eq!(s.list(&clock)[0].fire_at, utc(2026, 11, 9, 7, 0), "Monday");
    let fired = s.take_due(&clock, &zone);
    assert!(fired[0].repeats && !fired[0].missed);
    assert_eq!(
        s.list(&clock)[0].fire_at,
        utc(2026, 11, 10, 7, 0),
        "Tuesday"
    );
}

#[test]
fn a_local_time_zone_offset_is_respected() {
    let mut s = Scheduler::new(8);
    let zone = FixedOffset(-5 * 3600); // UTC-5
    let now = utc(2026, 1, 15, 0, 0); // 19:00 the previous local day
    s.set_alarm(now, &zone, 7, 0, None, "x").unwrap();
    let clock = StepClock::at(now);
    assert_eq!(s.list(&clock)[0].fire_at, utc(2026, 1, 15, 12, 0));
}

#[test]
fn invalid_alarm_times_are_rejected() {
    let mut s = Scheduler::new(8);
    let z = utc0();
    let now = 1_800_000_000;
    assert!(s.set_alarm(now, &z, 24, 0, None, "").is_err());
    assert!(s.set_alarm(now, &z, 7, 60, None, "").is_err());
    assert!(s.set_alarm(now, &z, 7, 0, Some(0), "").is_err());
    assert!(s.set_alarm(now, &z, 7, 0, Some(0b1000_0000), "").is_err());
}

#[test]
fn reminders_must_be_in_the_future() {
    let mut s = Scheduler::new(8);
    let clock = StepClock::at(utc(2026, 11, 7, 12, 0));
    let now = clock.now();
    assert!(s.set_reminder(now, now, "x").is_err());
    assert!(s.set_reminder(now, now - 1, "x").is_err());
    assert!(s.set_reminder(now, now + MAX_AHEAD_SECS + 1, "x").is_err());
    assert!(s.set_reminder_in(&clock, 0, "x").is_err());
    assert!(s.set_reminder_in(&clock, MAX_AHEAD_SECS + 1, "x").is_err());
    let id = s.set_reminder(now, now + 3600, "call mum").unwrap();
    assert_eq!(s.cancel(id).unwrap().label, "call mum");
    assert_eq!(s.cancel(id), Err(SchedError::NotFound));
}

// REQ-TMR-006, changed by this work: only calendar items wait for the wall clock.
#[test]
fn calendar_items_wait_for_a_valid_wall_clock_but_a_countdown_does_not() {
    let mut s = Scheduler::new(8);
    let zone = utc0();
    let clock = StepClock::at(T0);
    s.set_timer(&clock, 10, "timer").unwrap();
    s.set_reminder_in(&clock, 10, "in ten").unwrap();
    s.set_alarm(clock.now(), &zone, 12, 0, Some(days::ALL), "alarm")
        .unwrap();
    s.set_reminder(clock.now(), clock.now() + 10, "at").unwrap();
    clock.advance(100);
    clock.set_unset(true);
    let due = s.take_due(&clock, &zone);
    let mut labels: Vec<_> = due.iter().map(|f| f.label.as_str()).collect();
    labels.sort_unstable();
    assert_eq!(labels, ["in ten", "timer"], "countdowns need no calendar");
    assert_eq!(s.list(&clock).len(), 2, "the calendar items are pending");
    clock.set_unset(false);
    let again = s.take_due(&clock, &zone);
    assert_eq!(again.len(), 1, "the reminder rings once the clock is valid");
    assert_eq!(again[0].label, "at");
}

// REQ-TMR-005: calendar items follow the wall clock, so a corrected clock moves them with it.
#[test]
fn alarms_and_reminders_still_follow_the_wall_clock() {
    let zone = utc0();
    let mut s = Scheduler::new(8);
    let clock = StepClock::at(utc(2026, 11, 9, 5, 0));
    s.set_alarm(clock.now(), &zone, 7, 30, None, "wake")
        .unwrap();
    s.set_reminder(clock.now(), utc(2026, 11, 9, 8, 0), "call")
        .unwrap();
    clock.advance(60);
    assert!(s.take_due(&clock, &zone).is_empty());
    // Network time arrives and moves the clock two hours forward: 07:01. Nothing is due yet.
    clock.step_wall(2 * 3600);
    assert!(s.take_due(&clock, &zone).is_empty());
    // 29 minutes of real time later it is 07:30 on the corrected clock: the alarm rings, on time.
    clock.advance(29 * 60);
    let due = s.take_due(&clock, &zone);
    assert_eq!(due.len(), 1);
    assert_eq!((due[0].label.as_str(), due[0].late_secs), ("wake", 0));
    // A step back that undoes the correction keeps the reminder pending until 08:00 by the wall.
    clock.set_wall(utc(2026, 11, 9, 7, 59));
    assert!(s.take_due(&clock, &zone).is_empty());
    clock.advance(60);
    assert_eq!(s.take_due(&clock, &zone).len(), 1);
}

#[test]
fn items_found_long_overdue_after_a_restart_are_marked_missed() {
    let mut s = Scheduler::new(8);
    let zone = utc0();
    let t0 = utc(2026, 11, 7, 6, 0);
    s.set_alarm(t0, &zone, 7, 0, Some(days::ALL), "wake")
        .unwrap();
    // The device was off all day.
    let clock = StepClock::at(utc(2026, 11, 7, 15, 0));
    let fired = s.take_due(&clock, &zone);
    assert!(fired[0].missed);
    assert_eq!(fired[0].late_secs, 8 * 3600);
    assert_eq!(
        s.list(&clock)[0].fire_at,
        utc(2026, 11, 8, 7, 0),
        "next one is still scheduled"
    );
}

#[test]
fn snooze_creates_a_one_shot_with_the_same_label_that_runs_on_the_monotonic_clock() {
    let mut s = Scheduler::new(8);
    let zone = utc0();
    let t0 = utc(2026, 11, 7, 6, 0);
    s.set_alarm(t0, &zone, 7, 0, None, "wake").unwrap();
    let clock = StepClock::at(utc(2026, 11, 7, 7, 0));
    let fired = s.take_due(&clock, &zone).remove(0);
    s.snooze(&clock, &fired, 540).unwrap();
    assert_eq!(s.list(&clock)[0].fire_at, utc(2026, 11, 7, 7, 9));
    assert_eq!(s.list(&clock)[0].label, "wake");
    assert!(s.snooze(&clock, &fired, 5).is_err());
    // A step of the wall clock neither brings the snooze forward nor pushes it back.
    clock.step_wall(-3 * 3600);
    clock.advance(539);
    assert!(s.take_due(&clock, &zone).is_empty());
    clock.advance(1);
    assert_eq!(s.take_due(&clock, &zone).len(), 1);
}

#[test]
fn state_survives_a_restart() {
    let dir = TempDir::new("sched");
    let store = JsonStore::new(dir.0.join("sched.json"), SCHEMA);
    let t0 = utc(2026, 11, 7, 12, 0);
    let clock = StepClock::at(t0);
    let mut s = Scheduler::new(8);
    s.set_timer(&clock, 600, "pasta").unwrap();
    s.set_reminder(t0, t0 + 7200, "call mum").unwrap();
    s.save(&store, &clock).unwrap();
    let restored = Scheduler::load(&store, 8, &clock).unwrap();
    assert_eq!(restored.list(&clock).len(), 2);
    let mut restored = restored;
    let id = restored.set_timer(&clock, 5, "new").unwrap();
    assert_eq!(id, 3, "ids keep counting after a restart");
}

/// The scheduler as a new process sees it after `down` seconds off: a new monotonic origin, and the wall
/// clock `down` seconds later (or wherever the test puts it).
fn restart(store: &JsonStore, saved: &StepClock, down: i64) -> (Scheduler, StepClock) {
    let after = StepClock::with_mono(saved.now() + down, 7);
    (Scheduler::load(store, 8, &after).unwrap(), after)
}

// REQ-TMR-003
#[test]
fn a_restart_takes_the_time_the_program_was_off_off_a_countdown() {
    let dir = TempDir::new("sched");
    let store = JsonStore::new(dir.0.join("sched.json"), SCHEMA);
    let clock = StepClock::at(T0);
    let mut s = Scheduler::new(8);
    let id = s.set_timer(&clock, 600, "pasta").unwrap();
    clock.advance(100);
    s.save(&store, &clock).unwrap();
    let (r, after) = restart(&store, &clock, 30);
    assert_eq!(
        r.remaining(id, &after).unwrap(),
        470,
        "600 - 100 run - 30 off"
    );
    let json = std::fs::read_to_string(store.path()).unwrap();
    assert!(json.contains("\"remaining_secs\":500") && json.contains("\"saved_at\""));
    assert!(
        json.contains("\"fire_at\""),
        "still written for an older build"
    );
}

#[test]
fn the_remaining_time_is_worked_out_again_at_every_save() {
    let dir = TempDir::new("sched");
    let store = JsonStore::new(dir.0.join("sched.json"), SCHEMA);
    let clock = StepClock::at(T0);
    let mut s = Scheduler::new(8);
    s.set_timer(&clock, 600, "").unwrap();
    s.save(&store, &clock).unwrap();
    clock.advance(250);
    // Saved again with the timer unchanged: the figure in the file must follow the clock.
    s.save(&store, &clock).unwrap();
    assert!(
        std::fs::read_to_string(store.path())
            .unwrap()
            .contains("\"remaining_secs\":350")
    );
    // The bytes handed to the background writer say the same.
    let bytes = s.encode(&store, &clock).unwrap();
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("\"remaining_secs\":350")
    );
}

#[test]
fn a_wall_clock_that_is_behind_the_save_or_not_set_takes_nothing_off() {
    let dir = TempDir::new("sched");
    let store = JsonStore::new(dir.0.join("sched.json"), SCHEMA);
    let clock = StepClock::at(T0);
    let mut s = Scheduler::new(8);
    let id = s.set_timer(&clock, 600, "").unwrap();
    s.save(&store, &clock).unwrap();
    // Stale clock at boot, 3 h before the save.
    let (r, after) = restart(&store, &clock, -3 * 3600);
    assert_eq!(
        r.remaining(id, &after).unwrap(),
        600,
        "never a negative gap"
    );
    // A clock that has never been set (1970).
    let unset = StepClock::with_mono(40, 7);
    let r = Scheduler::load(&store, 8, &unset).unwrap();
    assert_eq!(r.remaining(id, &unset).unwrap(), 600);
}

#[test]
fn a_countdown_that_ran_out_while_the_program_was_off_comes_back_late() {
    let dir = TempDir::new("sched");
    let store = JsonStore::new(dir.0.join("sched.json"), SCHEMA);
    let clock = StepClock::at(T0);
    let mut s = Scheduler::new(8);
    s.set_timer(&clock, 600, "short").unwrap();
    s.set_timer(&clock, 3600, "long").unwrap();
    s.save(&store, &clock).unwrap();
    // Off for 20 minutes: "short" ran out 10 minutes ago (missed), "long" has 40 minutes left.
    let (mut r, after) = restart(&store, &clock, 1200);
    let due = r.take_due(&after, &utc0());
    assert_eq!(due.len(), 1);
    assert_eq!((due[0].label.as_str(), due[0].late_secs), ("short", 600));
    assert!(due[0].missed);
    // Off for 4 minutes past the end: late, but inside the grace, so it rings.
    let (mut r, after) = restart(&store, &clock, 840);
    let due = r.take_due(&after, &utc0());
    assert!(due.len() == 1 && !due[0].missed && due[0].late_secs == 240);
}

// The file as the previous version wrote it: an absolute fire_at, no remaining_secs.
#[test]
fn a_file_written_before_countdowns_were_kept_apart_still_loads() {
    let dir = TempDir::new("sched");
    let path = dir.0.join("sched.json");
    let old = format!(
        r#"{{"schema":1,"data":{{"next_id":3,"items":[
        {{"id":1,"kind":"timer","label":"pasta","fire_at":{t},"repeat":null,"created_at":{c}}},
        {{"id":2,"kind":"alarm","label":"wake","fire_at":{a},"repeat":null,"created_at":{c}}}]}}}}"#,
        t = T0 + 300,
        a = T0 + 2 * 86_400,
        c = T0 - 300
    );
    std::fs::write(&path, old).unwrap();
    let store = JsonStore::new(&path, SCHEMA);
    let clock = StepClock::with_mono(T0 + 100, 9);
    let mut s = Scheduler::load(&store, 8, &clock).unwrap();
    assert_eq!(s.remaining(1, &clock).unwrap(), 200, "fire_at - wall now");
    assert!(s.list(&clock)[0].is_countdown());
    assert!(
        !s.list(&clock)[1].is_countdown(),
        "an alarm stays on the calendar"
    );
    // The timer now runs on the monotonic clock, whatever the wall does.
    clock.step_wall(5 * 3600);
    clock.advance(199);
    assert!(s.take_due(&clock, &utc0()).is_empty());
    clock.advance(1);
    let due = s.take_due(&clock, &utc0());
    assert_eq!(due.len(), 1, "the alarm two days ahead is not due");
    assert!(
        due.iter()
            .any(|f| f.label == "pasta" && !f.missed && f.late_secs == 0)
    );
    // The next save writes the new form.
    let s = Scheduler::load(&store, 8, &clock).unwrap();
    s.save(&store, &clock).unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("remaining_secs")
    );
}

#[test]
fn an_old_file_with_an_overdue_timer_is_late_by_the_same_amount() {
    let dir = TempDir::new("sched");
    let path = dir.0.join("sched.json");
    let old = format!(
        r#"{{"schema":1,"data":{{"next_id":2,"items":[
        {{"id":1,"kind":"timer","label":"","fire_at":{t},"repeat":null,"created_at":{c}}}]}}}}"#,
        t = T0 - 900,
        c = T0 - 1500
    );
    std::fs::write(&path, old).unwrap();
    let clock = StepClock::at(T0);
    let mut s = Scheduler::load(&JsonStore::new(&path, SCHEMA), 8, &clock).unwrap();
    let due = s.take_due(&clock, &utc0());
    assert!(due.len() == 1 && due[0].missed && due[0].late_secs == 900);
}

#[cfg(feature = "tz")]
#[test]
fn a_daily_alarm_stays_at_local_time_across_a_daylight_saving_change() {
    use jiff::Timestamp;
    let zone = IanaZone::new("America/New_York").unwrap();
    let at = |s: &str| s.parse::<Timestamp>().unwrap().as_second();
    let mut s = Scheduler::new(8);
    // Saturday 2026-03-07 evening, before clocks go forward on Sunday 2026-03-08.
    let now = at("2026-03-07T20:00:00-05:00");
    s.set_alarm(now, &zone, 7, 0, Some(days::ALL), "wake")
        .unwrap();
    let clock = StepClock::at(now);
    assert_eq!(
        s.list(&clock)[0].fire_at,
        at("2026-03-08T07:00:00-04:00"),
        "07:00 EDT, 11:00Z"
    );
    clock.set_wall(at("2026-03-08T07:00:00-04:00"));
    s.take_due(&clock, &zone);
    assert_eq!(s.list(&clock)[0].fire_at, at("2026-03-09T07:00:00-04:00"));
    // The gap itself: 02:30 does not exist on 2026-03-08; the alarm lands just after the gap.
    let mut s2 = Scheduler::new(8);
    s2.set_alarm(at("2026-03-07T20:00:00-05:00"), &zone, 2, 30, None, "gap")
        .unwrap();
    assert_eq!(s2.list(&clock)[0].fire_at, at("2026-03-08T03:30:00-04:00"));
}

// REQ-TMR-011
#[test]
fn a_saved_scheduler_from_before_the_stopwatch_still_loads_and_the_stopwatch_is_saved() {
    let old = r#"{"next_id":2,"items":[{"id":1,"kind":"timer","label":"","fire_at":5,"repeat":null,"created_at":1}]}"#;
    let state: SchedState = serde_json::from_str(old).unwrap();
    assert!(state.stopwatch.is_none() && state.items.len() == 1);
    let mut s = Scheduler {
        state,
        max_items: 4,
    };
    assert!(!s.stopwatch_reset(), "nothing to reset");
    let clock = StepClock::at(1_800_000_000);
    s.stopwatch_start(&clock).unwrap();
    let json = serde_json::to_string(&s.persisted(&clock)).unwrap();
    assert!(json.contains("stopwatch"));
    let back: SchedState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s.persisted(&clock));
    // A stopwatch needs no timer slot, and a full scheduler can still run one.
    let mut full = Scheduler::new(1);
    full.set_timer(&StepClock::at(1_800_000_000), 60, "")
        .unwrap();
    assert!(full.stopwatch_start(&StepClock::at(1_800_000_000)).is_ok());
}
