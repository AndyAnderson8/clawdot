//! Portal management of timers, alarms, and reminders (REQ-CFG-027, REQ-TMR-008).
//!
//! The portal talks to the same scheduler the voice commands use, so there is one list and one persisted
//! file. Times for reminders come as a local date and time in the *device* time zone, which keeps daylight
//! saving correct (REQ-TMR-005); the browser never has to know the offset.

use super::Assistant;
use crate::app::{Fail, SchedOp};
use crate::sched::{Clock, Item, Kind, SchedError, civil_from_days, days, days_from_civil};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_LABEL_CHARS: usize = 100;

/// What the page sends to add or replace an item.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    /// `timer`, `alarm`, or `reminder`.
    kind: String,
    #[serde(default)]
    label: String,
    /// Timer length in seconds, or the delay of a reminder after a delay.
    seconds: Option<i64>,
    /// Alarm local hour and minute.
    hour: Option<u8>,
    minute: Option<u8>,
    /// Alarm weekdays, 0 = Monday to 6 = Sunday. Empty or missing: rings once.
    days: Option<Vec<u8>>,
    /// Reminder local date `YYYY-MM-DD` and time `HH:MM`, in the device time zone. A reminder
    /// takes these or `seconds`, not both.
    date: Option<String>,
    time: Option<String>,
}

fn invalid(m: &str) -> Fail {
    Fail::Invalid(m.to_owned())
}

fn sched_fail(e: &SchedError) -> Fail {
    match e {
        SchedError::TooMany => invalid(
            "There are too many timers, alarms, and reminders. Cancel one you no longer need.",
        ),
        SchedError::InvalidTime => invalid(
            "That time does not work. Timers run from 1 second to 7 days, and reminders must be in the future and within a year.",
        ),
        SchedError::NotFound => {
            Fail::NotFound("That timer, alarm, or reminder no longer exists.".into())
        }
    }
}

fn parse_date(s: &str) -> Option<(i64, i64, i64)> {
    let mut p = s.split('-');
    let (y, m, d) = (p.next()?, p.next()?, p.next()?);
    if p.next().is_some() || y.len() != 4 {
        return None;
    }
    let (y, m, d) = (
        y.parse::<i64>().ok()?,
        m.parse::<i64>().ok()?,
        d.parse::<i64>().ok()?,
    );
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

fn parse_time(s: &str) -> Option<(u8, u8)> {
    let (h, m) = s.split_once(':')?;
    let (h, m) = (h.parse::<u8>().ok()?, m.parse::<u8>().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

fn day_mask(list: &[u8]) -> Result<Option<u8>, Fail> {
    if list.is_empty() {
        return Ok(None);
    }
    let mut mask = 0u8;
    for d in list {
        if *d > 6 {
            return Err(invalid("Days run from 0 (Monday) to 6 (Sunday)."));
        }
        mask |= 1 << d;
    }
    Ok(Some(mask))
}

/// One item for the portal. `fire_at` and `seconds_left` are worked out now (a countdown's wall-clock time is
/// derived from the seconds it has left), so they stay right after the wall clock was set.
fn item_json(i: &Item, clock: &dyn Clock) -> Value {
    let kind = match i.kind {
        Kind::Timer => "timer",
        Kind::Alarm => "alarm",
        Kind::Reminder => "reminder",
    };
    let repeat = i.repeat.map(|r| {
        let list: Vec<u8> = (0..7u8).filter(|d| r.days & (1 << d) != 0).collect();
        json!({ "days": list, "hour": r.hour, "minute": r.minute, "every_day": r.days & days::ALL == days::ALL })
    });
    json!({
        "id": i.id, "kind": kind, "label": i.label, "fire_at": i.due_wall(clock),
        "seconds_left": i.secs_left(clock), "repeat": repeat, "countdown": i.is_countdown(),
    })
}

impl Assistant {
    /// Carry out a portal request against the scheduler and return the whole list afterwards.
    ///
    /// # Errors
    /// [`Fail::Invalid`] for a bad request, [`Fail::NotFound`] for an unknown item.
    pub fn admin_sched(&mut self, op: SchedOp) -> Result<Value, Fail> {
        let changed = match op {
            SchedOp::List => None,
            SchedOp::Add(v) => Some(self.add_item(&v)?),
            SchedOp::Edit(id, v) => {
                let old = self.sched.cancel(id).map_err(|e| sched_fail(&e))?;
                match self.add_item(&v) {
                    Ok(new) => {
                        self.sched.renumber(new, id);
                        Some(id)
                    }
                    Err(e) => {
                        self.sched.restore(old);
                        return Err(e);
                    }
                }
            }
            SchedOp::Cancel(id) => {
                self.sched.cancel(id).map_err(|e| sched_fail(&e))?;
                Some(id)
            }
        };
        if changed.is_some() {
            self.save();
        }
        let now = self.clock.now();
        Ok(json!({
            "id": changed,
            "now": now,
            "clock_valid": self.clock.wall_clock_valid(),
            "items": self.sched.list(&*self.clock).into_iter().map(|i| item_json(i, &*self.clock)).collect::<Vec<_>>(),
        }))
    }

    fn add_item(&mut self, v: &Value) -> Result<u64, Fail> {
        let spec: Spec = serde_json::from_value(v.clone())
            .map_err(|_| invalid("The request is not a valid timer, alarm, or reminder."))?;
        // A countdown (a timer, or a reminder after a delay) needs no calendar, so it can be set before the wall
        // clock is (REQ-TMR-009).
        let countdown = spec.kind == "timer" || (spec.kind == "reminder" && spec.seconds.is_some());
        if !self.clock.wall_clock_valid() && !countdown {
            return Err(invalid(
                "The device clock is not set yet, so no alarm or reminder at a time can be set. It needs to get the time from the network. A timer or a reminder after a delay still works.",
            ));
        }
        let label = spec.label.trim();
        if label.chars().count() > MAX_LABEL_CHARS {
            return Err(invalid("Keep the name or text under 100 characters."));
        }
        let now = self.clock.now();
        match spec.kind.as_str() {
            "timer" => {
                let secs = spec
                    .seconds
                    .ok_or_else(|| invalid("A timer needs a length."))?;
                self.sched
                    .set_timer(&*self.clock, secs, label)
                    .map_err(|e| sched_fail(&e))
            }
            "alarm" => {
                let (Some(h), Some(m)) = (spec.hour, spec.minute) else {
                    return Err(invalid("An alarm needs a time of day."));
                };
                let mask = day_mask(spec.days.as_deref().unwrap_or(&[]))?;
                self.sched
                    .set_alarm(now, &*self.zone, h, m, mask, label)
                    .map_err(|e| sched_fail(&e))
            }
            "reminder" => self.add_reminder(&spec, label, now),
            _ => Err(invalid("The kind must be timer, alarm, or reminder.")),
        }
    }

    /// A reminder, after a delay or at a date and time in the device's zone, with some text to say.
    fn add_reminder(
        &mut self,
        spec: &Spec,
        label: &str,
        now: crate::sched::Ts,
    ) -> Result<u64, Fail> {
        if label.is_empty() {
            return Err(invalid("A reminder needs some text to say."));
        }
        if let Some(secs) = spec.seconds {
            if spec.date.is_some() || spec.time.is_some() {
                return Err(invalid(
                    "Give a reminder either a delay or a date and a time, not both.",
                ));
            }
            return self
                .sched
                .set_reminder_in(&*self.clock, secs, label)
                .map_err(|e| sched_fail(&e));
        }
        let (Some(date), Some(time)) = (spec.date.as_deref(), spec.time.as_deref()) else {
            return Err(invalid("A reminder needs a delay, or a date and a time."));
        };
        let (Some((y, mo, d)), Some((h, mi))) = (parse_date(date), parse_time(time)) else {
            return Err(invalid("Use a date like 2026-11-09 and a time like 15:30."));
        };
        let day = days_from_civil(y, mo, d);
        if civil_from_days(day) != (y, mo, d) {
            return Err(invalid("That date does not exist."));
        }
        let at = self
            .zone
            .local_to_utc(day, h, mi)
            .ok_or_else(|| invalid("That date and time do not exist."))?;
        self.sched
            .set_reminder(now, at, label)
            .map_err(|e| sched_fail(&e))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::sched::{FixedOffset, Scheduler, Ts};
    use std::sync::Arc;

    struct At(Ts);
    impl Clock for At {
        fn monotonic_secs(&self) -> i64 {
            self.now()
        }
        fn now(&self) -> Ts {
            self.0
        }
    }

    /// 2026-11-09 12:00 UTC; the device zone is UTC-6.
    const NOW: Ts = 1_794_225_600;

    fn assistant(now: Ts) -> Assistant {
        Assistant::new(
            crate::lang::DEFAULT,
            Arc::new(At(now)),
            Box::new(FixedOffset(-6 * 3600)),
            Scheduler::new(4),
            None,
            None,
        )
    }

    fn ids(v: &Value) -> Vec<u64> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_u64().unwrap())
            .collect()
    }

    // REQ-CFG-027
    #[test]
    fn a_timer_an_alarm_and_a_reminder_can_be_added_listed_and_cancelled() {
        let mut a = assistant(NOW);
        let t = a
            .admin_sched(SchedOp::Add(
                json!({"kind":"timer","seconds":600,"label":"tea"}),
            ))
            .unwrap();
        assert_eq!(t["items"][0]["seconds_left"], 600);
        a.admin_sched(SchedOp::Add(
            json!({"kind":"alarm","hour":7,"minute":30,"days":[0,1,2,3,4],"label":"work"}),
        ))
        .unwrap();
        let r = a
            .admin_sched(SchedOp::Add(
                json!({"kind":"reminder","date":"2026-11-10","time":"09:15","label":"call mum"}),
            ))
            .unwrap();
        assert_eq!(ids(&r).len(), 3);
        // 09:15 at UTC-6 on 10 November is 15:15 UTC.
        let rem = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["kind"] == "reminder")
            .unwrap();
        assert_eq!(rem["fire_at"], NOW + 86_400 + 3 * 3600 + 15 * 60);
        let first = r["id"].as_u64().unwrap();
        let after = a.admin_sched(SchedOp::Cancel(first)).unwrap();
        assert_eq!(ids(&after).len(), 2);
        assert!(!ids(&after).contains(&first));
    }

    // REQ-CFG-027, REQ-TMR-005
    #[test]
    fn an_alarm_repeats_on_the_chosen_days_in_device_time() {
        let mut a = assistant(NOW);
        let r = a
            .admin_sched(SchedOp::Add(
                json!({"kind":"alarm","hour":7,"minute":0,"days":[5,6],"label":""}),
            ))
            .unwrap();
        assert_eq!(r["items"][0]["repeat"]["days"], json!([5, 6]));
        assert_eq!(r["items"][0]["repeat"]["hour"], 7);
        // Next Saturday 07:00 at UTC-6 is 13:00 UTC, on 14 November.
        assert_eq!(r["items"][0]["fire_at"], 1_794_225_600 + 5 * 86_400 + 3600);
    }

    // REQ-CFG-027
    #[test]
    fn an_edit_keeps_the_id_and_a_failed_edit_changes_nothing() {
        let mut a = assistant(NOW);
        let id = a
            .admin_sched(SchedOp::Add(
                json!({"kind":"timer","seconds":60,"label":"a"}),
            ))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let r = a
            .admin_sched(SchedOp::Edit(
                id,
                json!({"kind":"timer","seconds":120,"label":"b"}),
            ))
            .unwrap();
        assert_eq!(ids(&r), vec![id]);
        assert_eq!(r["items"][0]["label"], "b");
        assert_eq!(r["items"][0]["seconds_left"], 120);
        let bad = a.admin_sched(SchedOp::Edit(id, json!({"kind":"timer","seconds":0})));
        assert!(matches!(bad, Err(Fail::Invalid(_))));
        let r = a.admin_sched(SchedOp::List).unwrap();
        assert_eq!(r["items"][0]["label"], "b");
        assert!(matches!(
            a.admin_sched(SchedOp::Edit(99, json!({"kind":"timer","seconds":5}))),
            Err(Fail::NotFound(_))
        ));
    }

    // REQ-CFG-027
    #[test]
    fn bad_requests_say_what_to_fix() {
        let mut a = assistant(NOW);
        for body in [
            json!({"kind":"timer"}),
            json!({"kind":"alarm","hour":25,"minute":0}),
            json!({"kind":"alarm","hour":7,"minute":0,"days":[9]}),
            json!({"kind":"reminder","label":"x","date":"2026-02-31","time":"10:00"}),
            json!({"kind":"reminder","label":"x","date":"2026-11-01","time":"10:00"}),
            json!({"kind":"reminder","date":"2026-11-20","time":"10:00"}),
            json!({"kind":"toast"}),
            json!({"kind":"timer","seconds":5,"extra":1}),
        ] {
            let e = a.admin_sched(SchedOp::Add(body.clone()));
            assert!(matches!(e, Err(Fail::Invalid(_))), "{body}: {e:?}");
        }
        assert_eq!(ids(&a.admin_sched(SchedOp::List).unwrap()).len(), 0);
    }

    // REQ-TMR-009: a reminder after a delay is a countdown, set from the portal with `seconds`.
    #[test]
    fn a_reminder_after_a_delay_is_a_countdown_and_works_without_the_clock() {
        let mut a = assistant(5);
        let r = a
            .admin_sched(SchedOp::Add(
                json!({"kind":"reminder","seconds":600,"label":"stand up"}),
            ))
            .unwrap();
        assert_eq!(r["clock_valid"], false);
        assert_eq!(r["items"][0]["kind"], "reminder");
        assert_eq!(r["items"][0]["seconds_left"], 600);
        assert_eq!(r["items"][0]["countdown"], true);
        // The same with a set clock.
        let mut b = assistant(NOW);
        let r = b
            .admin_sched(SchedOp::Add(
                json!({"kind":"reminder","seconds":90,"label":"x"}),
            ))
            .unwrap();
        assert_eq!(r["items"][0]["fire_at"], NOW + 90);
        // A reminder at a time is not a countdown.
        let r = b
            .admin_sched(SchedOp::Add(
                json!({"kind":"reminder","date":"2026-11-10","time":"09:15","label":"y"}),
            ))
            .unwrap();
        let at = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["label"] == "y")
            .unwrap();
        assert_eq!(at["countdown"], false);
    }

    // REQ-TMR-009, REQ-CFG-027: a reminder after a delay needs text, a sane delay, and not a date as well; an
    // edit can switch between the two forms.
    #[test]
    fn a_reminder_after_a_delay_is_checked_and_can_be_edited() {
        let mut a = assistant(NOW);
        for body in [
            json!({"kind":"reminder","seconds":60}),
            json!({"kind":"reminder","seconds":0,"label":"x"}),
            json!({"kind":"reminder","seconds":-5,"label":"x"}),
            json!({"kind":"reminder","seconds":60,"date":"2026-11-10","time":"09:00","label":"x"}),
        ] {
            let e = a.admin_sched(SchedOp::Add(body.clone()));
            assert!(matches!(e, Err(Fail::Invalid(_))), "{body}: {e:?}");
        }
        let id = a
            .admin_sched(SchedOp::Add(
                json!({"kind":"reminder","seconds":60,"label":"a"}),
            ))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let r = a
            .admin_sched(SchedOp::Edit(
                id,
                json!({"kind":"reminder","date":"2026-11-10","time":"09:15","label":"a"}),
            ))
            .unwrap();
        assert_eq!(ids(&r), vec![id]);
        assert_eq!(r["items"][0]["countdown"], false);
        let r = a
            .admin_sched(SchedOp::Edit(
                id,
                json!({"kind":"reminder","seconds":300,"label":"a"}),
            ))
            .unwrap();
        assert_eq!(r["items"][0]["countdown"], true);
        assert_eq!(r["items"][0]["seconds_left"], 300);
    }

    // REQ-TMR-006, REQ-TMR-009: a point on the calendar needs the calendar; a countdown does not.
    #[test]
    fn no_alarm_or_reminder_can_be_added_while_the_clock_is_unset_but_a_timer_can() {
        let mut a = assistant(5);
        for body in [
            json!({"kind":"alarm","hour":7,"minute":0}),
            json!({"kind":"reminder","label":"x","date":"2026-11-10","time":"09:00"}),
        ] {
            let e = a.admin_sched(SchedOp::Add(body));
            assert!(matches!(e, Err(Fail::Invalid(m)) if m.contains("clock")));
        }
        let r = a
            .admin_sched(SchedOp::Add(json!({"kind":"timer","seconds":5})))
            .unwrap();
        assert_eq!(r["clock_valid"], false);
        assert_eq!(r["items"][0]["seconds_left"], 5);
    }
}
