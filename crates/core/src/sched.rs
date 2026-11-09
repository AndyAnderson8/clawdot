//! Timers, alarms, and reminders (REQ-TMR-001..008).
//!
//! All deadlines are UTC unix seconds. Alarm recurrence is expressed in *local* wall time through the
//! [`Zone`] trait, so "every weekday at 07:00" stays at 07:00 across daylight-saving changes
//! (REQ-TMR-005). Nothing fires until the wall clock is trustworthy (REQ-TMR-006), and the whole state
//! is a small serialisable value that survives restarts (REQ-TMR-003).

use crate::stopwatch::{Lap, Reading, Refusal, Stopwatch};
use crate::store::{JsonStore, StoreError};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Seconds since the Unix epoch (UTC).
pub type Ts = i64;

/// Anything older than this means the clock has not been set yet (no RTC, no NTP).
const CLOCK_VALID_AFTER: Ts = 1_700_000_000;
/// Items later than this are reported as missed rather than ringing as if on time.
pub const MISSED_GRACE_SECS: i64 = 300;
/// Longest timer.
pub const MAX_TIMER_SECS: i64 = 7 * 24 * 3600;
/// Furthest ahead a reminder or alarm may be set.
pub const MAX_AHEAD_SECS: i64 = 366 * 24 * 3600;

/// Source of the current time.
pub trait Clock {
    /// Current UTC unix time.
    fn now(&self) -> Ts;
    /// False until the clock has been synchronised (REQ-TMR-006).
    fn wall_clock_valid(&self) -> bool {
        self.now() >= CLOCK_VALID_AFTER
    }
}

/// The operating system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Ts {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or(0)
    }
}

/// Local-time rules needed to place alarms on the calendar.
pub trait Zone {
    /// The local calendar day (days since 1970-01-01, in local time) that contains `ts`.
    fn local_day(&self, ts: Ts) -> i64;
    /// The instant of local `hour:minute` on local day `day`. Wall times that do not exist (the hour
    /// skipped by a daylight-saving jump) resolve to the instant just after the gap.
    fn local_to_utc(&self, day: i64, hour: u8, minute: u8) -> Option<Ts>;

    /// Minutes since local midnight at `ts` (0 to 1439).
    fn local_minute_of_day(&self, ts: Ts) -> u16 {
        let day = self.local_day(ts);
        let start = self.local_to_utc(day, 0, 0).unwrap_or(day * 86_400);
        u16::try_from((ts - start).div_euclid(60).clamp(0, 1439)).unwrap_or(0)
    }
}

/// A fixed offset from UTC, in seconds east.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixedOffset(pub i32);

impl Zone for FixedOffset {
    fn local_day(&self, ts: Ts) -> i64 {
        (ts + i64::from(self.0)).div_euclid(86_400)
    }
    fn local_to_utc(&self, day: i64, hour: u8, minute: u8) -> Option<Ts> {
        Some(day * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 - i64::from(self.0))
    }
}

/// An IANA zone such as `America/New_York`, with correct daylight-saving behaviour.
#[cfg(feature = "tz")]
#[derive(Debug, Clone)]
pub struct IanaZone(jiff::tz::TimeZone);

#[cfg(feature = "tz")]
impl IanaZone {
    /// Look a zone up by name.
    ///
    /// # Errors
    /// The name is unknown.
    pub fn new(name: &str) -> Result<Self, String> {
        jiff::tz::TimeZone::get(name)
            .map(Self)
            .map_err(|e| format!("unknown time zone `{name}`: {e}"))
    }
}

#[cfg(feature = "tz")]
impl Zone for IanaZone {
    fn local_day(&self, ts: Ts) -> i64 {
        match jiff::Timestamp::from_second(ts) {
            Ok(t) => {
                let z = t.to_zoned(self.0.clone());
                days_from_civil(
                    i64::from(z.year()),
                    i64::from(z.month()),
                    i64::from(z.day()),
                )
            }
            Err(_) => ts.div_euclid(86_400),
        }
    }
    fn local_to_utc(&self, day: i64, hour: u8, minute: u8) -> Option<Ts> {
        let (y, m, d) = civil_from_days(day);
        let dt = jiff::civil::DateTime::new(
            i16::try_from(y).ok()?,
            i8::try_from(m).ok()?,
            i8::try_from(d).ok()?,
            i8::try_from(hour).ok()?,
            i8::try_from(minute).ok()?,
            0,
            0,
        )
        .ok()?;
        self.0.to_zoned(dt).ok().map(|z| z.timestamp().as_second())
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
#[must_use]
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The `(year, month, day)` of a day number from [`days_from_civil`].
#[must_use]
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Weekday of a local day number, Monday = 0 through Sunday = 6.
#[must_use]
pub fn weekday(day: i64) -> u8 {
    // 1970-01-01 was a Thursday (3 when Monday is 0).
    u8::try_from((day + 3).rem_euclid(7)).unwrap_or(0)
}

/// Bit mask of weekdays, bit 0 = Monday.
pub mod days {
    /// Every day.
    pub const ALL: u8 = 0b0111_1111;
    /// Monday to Friday.
    pub const WEEKDAYS: u8 = 0b0001_1111;
    /// Saturday and Sunday.
    pub const WEEKEND: u8 = 0b0110_0000;
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A countdown.
    Timer,
    /// A wall-clock alarm, possibly repeating.
    Alarm,
    /// A reminder with text.
    Reminder,
}

/// Repetition rule for an alarm: local `hour:minute` on the days in the mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repeat {
    /// Weekday bit mask (see [`days`]).
    pub days: u8,
    /// Local hour, 0 to 23.
    pub hour: u8,
    /// Local minute, 0 to 59.
    pub minute: u8,
}

/// A scheduled item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Stable identifier.
    pub id: u64,
    /// What it is.
    pub kind: Kind,
    /// Name or reminder text.
    pub label: String,
    /// Next firing instant.
    pub fire_at: Ts,
    /// Recurrence, for alarms.
    pub repeat: Option<Repeat>,
    /// When it was created.
    pub created_at: Ts,
}

/// An item that has come due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fired {
    /// The item's identifier.
    pub id: u64,
    /// What it is.
    pub kind: Kind,
    /// Name or reminder text.
    pub label: String,
    /// It will ring again (a repeating alarm).
    pub repeats: bool,
    /// How long after its deadline it was noticed.
    pub late_secs: i64,
    /// Later than [`MISSED_GRACE_SECS`]: announce as missed, do not ring.
    pub missed: bool,
}

/// Why a scheduling request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedError {
    /// The item limit is reached.
    TooMany,
    /// The time or duration is out of range.
    InvalidTime,
    /// No such item.
    NotFound,
}

impl fmt::Display for SchedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooMany => write!(f, "too many timers, alarms, and reminders"),
            Self::InvalidTime => write!(f, "that time is not valid"),
            Self::NotFound => write!(f, "no such timer, alarm, or reminder"),
        }
    }
}

impl std::error::Error for SchedError {}

/// The serialisable part of the scheduler.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedState {
    next_id: u64,
    items: Vec<Item>,
    /// The stopwatch, if there is one (REQ-TMR-011). Files from before it existed load without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stopwatch: Option<Stopwatch>,
}

/// Schema version of the persisted scheduler file.
pub const SCHEMA: u32 = 1;

/// Holds timers, alarms, and reminders.
#[derive(Debug, Clone)]
pub struct Scheduler {
    state: SchedState,
    max_items: usize,
}

impl Scheduler {
    /// An empty scheduler holding at most `max_items`.
    #[must_use]
    pub fn new(max_items: usize) -> Self {
        Self {
            state: SchedState::default(),
            max_items,
        }
    }

    /// Restore from a store.
    ///
    /// # Errors
    /// Storage errors (corrupt or newer-schema files are reported, never silently dropped).
    pub fn load(store: &JsonStore, max_items: usize) -> Result<Self, StoreError> {
        Ok(Self {
            state: store.load()?,
            max_items,
        })
    }

    /// Persist atomically.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore) -> Result<(), StoreError> {
        store.save(&self.state)
    }

    fn insert(&mut self, mut item: Item) -> Result<u64, SchedError> {
        if self.state.items.len() >= self.max_items {
            return Err(SchedError::TooMany);
        }
        self.state.next_id += 1;
        item.id = self.state.next_id;
        let id = item.id;
        self.state.items.push(item);
        Ok(id)
    }

    /// A countdown of `secs` seconds from `now`.
    ///
    /// # Errors
    /// [`SchedError::InvalidTime`] outside 1 second to 7 days; [`SchedError::TooMany`].
    pub fn set_timer(&mut self, now: Ts, secs: i64, label: &str) -> Result<u64, SchedError> {
        if !(1..=MAX_TIMER_SECS).contains(&secs) {
            return Err(SchedError::InvalidTime);
        }
        self.insert(Item {
            id: 0,
            kind: Kind::Timer,
            label: label.to_owned(),
            fire_at: now + secs,
            repeat: None,
            created_at: now,
        })
    }

    /// An alarm at local `hour:minute`. With `days` it repeats on those weekdays; without, it rings once
    /// at the next occurrence of that time.
    ///
    /// # Errors
    /// [`SchedError::InvalidTime`] for an impossible time or empty day mask; [`SchedError::TooMany`].
    pub fn set_alarm(
        &mut self,
        now: Ts,
        zone: &dyn Zone,
        hour: u8,
        minute: u8,
        days: Option<u8>,
        label: &str,
    ) -> Result<u64, SchedError> {
        if hour > 23 || minute > 59 || days == Some(0) || days.is_some_and(|d| d & !days::ALL != 0)
        {
            return Err(SchedError::InvalidTime);
        }
        let mask = days.unwrap_or(days::ALL);
        let fire_at =
            next_occurrence(zone, now, mask, hour, minute).ok_or(SchedError::InvalidTime)?;
        self.insert(Item {
            id: 0,
            kind: Kind::Alarm,
            label: label.to_owned(),
            fire_at,
            repeat: days.map(|d| Repeat {
                days: d,
                hour,
                minute,
            }),
            created_at: now,
        })
    }

    /// A reminder at the instant `at`.
    ///
    /// # Errors
    /// [`SchedError::InvalidTime`] when `at` is not in the future or is over a year away.
    pub fn set_reminder(&mut self, now: Ts, at: Ts, text: &str) -> Result<u64, SchedError> {
        if at <= now || at - now > MAX_AHEAD_SECS {
            return Err(SchedError::InvalidTime);
        }
        self.insert(Item {
            id: 0,
            kind: Kind::Reminder,
            label: text.to_owned(),
            fire_at: at,
            repeat: None,
            created_at: now,
        })
    }

    /// Ring again in `secs`: a one-shot alarm carrying the label of a fired item.
    ///
    /// # Errors
    /// [`SchedError::InvalidTime`] outside 1 minute to 1 hour; [`SchedError::TooMany`].
    pub fn snooze(&mut self, now: Ts, fired: &Fired, secs: i64) -> Result<u64, SchedError> {
        if !(60..=3600).contains(&secs) {
            return Err(SchedError::InvalidTime);
        }
        self.insert(Item {
            id: 0,
            kind: fired.kind,
            label: fired.label.clone(),
            fire_at: now + secs,
            repeat: None,
            created_at: now,
        })
    }

    /// Start the stopwatch at `now`. A stopped one is replaced; a running one is left alone.
    ///
    /// # Errors
    /// [`Refusal::Running`] with its reading, or [`Refusal::ClockWentBack`] when it is running and
    /// the clock is behind its last record.
    pub fn stopwatch_start(&mut self, now: Ts) -> Result<(), Refusal> {
        if let Some(sw) = &self.state.stopwatch {
            let r = sw.read(now)?;
            if r.running {
                return Err(Refusal::Running(r));
            }
        }
        self.state.stopwatch = Some(Stopwatch::start(now));
        Ok(())
    }

    /// Record a lap.
    ///
    /// # Errors
    /// [`Refusal`]; [`Refusal::None`] when there is no stopwatch.
    pub fn stopwatch_lap(&mut self, now: Ts) -> Result<Lap, Refusal> {
        self.state.stopwatch.as_mut().ok_or(Refusal::None)?.lap(now)
    }

    /// Stop the stopwatch.
    ///
    /// # Errors
    /// [`Refusal`]; [`Refusal::None`] when there is no stopwatch.
    pub fn stopwatch_stop(&mut self, now: Ts) -> Result<Reading, Refusal> {
        self.state
            .stopwatch
            .as_mut()
            .ok_or(Refusal::None)?
            .stop(now)
    }

    /// What the stopwatch shows, with each lap's own time.
    ///
    /// # Errors
    /// [`Refusal`]; [`Refusal::None`] when there is no stopwatch.
    pub fn stopwatch_read(&self, now: Ts) -> Result<(Reading, Vec<i64>), Refusal> {
        let sw = self.state.stopwatch.as_ref().ok_or(Refusal::None)?;
        Ok((sw.read(now)?, sw.splits()))
    }

    /// Throw the stopwatch away; false when there was none. Needs no clock.
    pub fn stopwatch_reset(&mut self) -> bool {
        self.state.stopwatch.take().is_some()
    }

    /// Remove an item.
    ///
    /// # Errors
    /// [`SchedError::NotFound`].
    pub fn cancel(&mut self, id: u64) -> Result<Item, SchedError> {
        let pos = self
            .state
            .items
            .iter()
            .position(|i| i.id == id)
            .ok_or(SchedError::NotFound)?;
        Ok(self.state.items.remove(pos))
    }

    /// Give item `from` the identifier `to` (used by an edit, which creates the new item before it is known to
    /// succeed). Does nothing when `from` does not exist or `to` is taken.
    pub fn renumber(&mut self, from: u64, to: u64) {
        if self.state.items.iter().any(|i| i.id == to) {
            return;
        }
        if let Some(i) = self.state.items.iter_mut().find(|i| i.id == from) {
            i.id = to;
        }
    }

    /// Put back an item taken out by [`Scheduler::cancel`] (an edit that failed). Ignored if its id is in use.
    pub fn restore(&mut self, item: Item) {
        if !self.state.items.iter().any(|i| i.id == item.id) {
            self.state.items.push(item);
        }
    }

    /// Remove every item of one kind; returns how many.
    pub fn cancel_kind(&mut self, kind: Kind) -> usize {
        let before = self.state.items.len();
        self.state.items.retain(|i| i.kind != kind);
        before - self.state.items.len()
    }

    /// All items, soonest first.
    #[must_use]
    pub fn list(&self) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.state.items.iter().collect();
        v.sort_by_key(|i| (i.fire_at, i.id));
        v
    }

    /// Seconds until the soonest item, for sleeping the event loop; `None` when empty.
    #[must_use]
    pub fn next_due_in(&self, now: Ts) -> Option<i64> {
        self.state
            .items
            .iter()
            .map(|i| (i.fire_at - now).max(0))
            .min()
    }

    /// Seconds left on a timer (or time until any item).
    ///
    /// # Errors
    /// [`SchedError::NotFound`].
    pub fn remaining(&self, id: u64, now: Ts) -> Result<i64, SchedError> {
        self.state
            .items
            .iter()
            .find(|i| i.id == id)
            .map(|i| (i.fire_at - now).max(0))
            .ok_or(SchedError::NotFound)
    }

    /// Collect everything that has come due. Returns nothing until the wall clock is valid
    /// (REQ-TMR-006). Repeating alarms are rescheduled; everything else is removed.
    pub fn take_due(&mut self, clock: &dyn Clock, zone: &dyn Zone) -> Vec<Fired> {
        if !clock.wall_clock_valid() {
            return Vec::new();
        }
        let now = clock.now();
        let mut fired = Vec::new();
        let mut keep = Vec::with_capacity(self.state.items.len());
        for mut item in std::mem::take(&mut self.state.items) {
            if item.fire_at > now {
                keep.push(item);
                continue;
            }
            let late = now - item.fire_at;
            fired.push(Fired {
                id: item.id,
                kind: item.kind,
                label: item.label.clone(),
                repeats: item.repeat.is_some(),
                late_secs: late,
                missed: late > MISSED_GRACE_SECS,
            });
            if let Some(rule) = item.repeat {
                if let Some(next) = next_occurrence(zone, now, rule.days, rule.hour, rule.minute) {
                    item.fire_at = next;
                    keep.push(item);
                }
            }
        }
        self.state.items = keep;
        fired.sort_by_key(|f| (std::cmp::Reverse(f.late_secs), f.id));
        fired
    }
}

/// The first instant strictly after `now` that is local `hour:minute` on a day in `mask`.
#[must_use]
pub fn next_occurrence(zone: &dyn Zone, now: Ts, mask: u8, hour: u8, minute: u8) -> Option<Ts> {
    let today = zone.local_day(now);
    for offset in 0..=8 {
        let day = today + offset;
        if mask & (1 << weekday(day)) == 0 {
            continue;
        }
        if let Some(ts) = zone.local_to_utc(day, hour, minute) {
            if ts > now {
                return Some(ts);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;
    use std::cell::Cell;

    struct FakeClock {
        now: Cell<Ts>,
        valid: Cell<bool>,
    }

    impl FakeClock {
        fn at(now: Ts) -> Self {
            Self {
                now: Cell::new(now),
                valid: Cell::new(true),
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Ts {
            self.now.get()
        }
        fn wall_clock_valid(&self) -> bool {
            self.valid.get()
        }
    }

    /// UTC instant for a calendar time.
    fn utc(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> Ts {
        days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60
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
        let id = s.set_timer(t0, 600, "pasta").unwrap();
        let clock = FakeClock::at(t0 + 599);
        assert!(s.take_due(&clock, &FixedOffset(0)).is_empty());
        assert_eq!(s.remaining(id, clock.now()).unwrap(), 1);
        clock.now.set(t0 + 600);
        let due = s.take_due(&clock, &FixedOffset(0));
        assert_eq!(due.len(), 1);
        assert_eq!(
            (due[0].kind, due[0].label.as_str(), due[0].late_secs),
            (Kind::Timer, "pasta", 0)
        );
        assert!(s.list().is_empty(), "fired one-shots are removed");
    }

    #[test]
    fn durations_are_validated() {
        let mut s = Scheduler::new(8);
        for bad in [0, -5, MAX_TIMER_SECS + 1] {
            assert_eq!(
                s.set_timer(1_800_000_000, bad, ""),
                Err(SchedError::InvalidTime)
            );
        }
        assert!(s.set_timer(1_800_000_000, MAX_TIMER_SECS, "").is_ok());
    }

    #[test]
    fn the_item_limit_is_enforced() {
        let mut s = Scheduler::new(2);
        s.set_timer(1_800_000_000, 10, "a").unwrap();
        s.set_timer(1_800_000_000, 10, "b").unwrap();
        assert_eq!(
            s.set_timer(1_800_000_000, 10, "c"),
            Err(SchedError::TooMany)
        );
    }

    #[test]
    fn a_one_off_alarm_rings_today_if_the_time_is_ahead_otherwise_tomorrow() {
        let mut s = Scheduler::new(8);
        let zone = FixedOffset(0);
        let now = utc(2026, 11, 7, 6, 0); // Saturday 06:00
        s.set_alarm(now, &zone, 7, 30, None, "wake").unwrap();
        assert_eq!(s.list()[0].fire_at, utc(2026, 11, 7, 7, 30));
        let later = utc(2026, 11, 7, 8, 0);
        s.set_alarm(later, &zone, 7, 30, None, "wake2").unwrap();
        assert_eq!(s.list()[1].fire_at, utc(2026, 11, 8, 7, 30));
    }

    #[test]
    fn weekday_alarms_skip_the_weekend_and_reschedule_after_firing() {
        let mut s = Scheduler::new(8);
        let zone = FixedOffset(0);
        let friday_evening = utc(2026, 11, 6, 20, 0);
        s.set_alarm(friday_evening, &zone, 7, 0, Some(days::WEEKDAYS), "work")
            .unwrap();
        assert_eq!(s.list()[0].fire_at, utc(2026, 11, 9, 7, 0), "Monday");
        let clock = FakeClock::at(utc(2026, 11, 9, 7, 0));
        let fired = s.take_due(&clock, &zone);
        assert!(fired[0].repeats && !fired[0].missed);
        assert_eq!(s.list()[0].fire_at, utc(2026, 11, 10, 7, 0), "Tuesday");
    }

    #[test]
    fn a_local_time_zone_offset_is_respected() {
        let mut s = Scheduler::new(8);
        let zone = FixedOffset(-5 * 3600); // UTC-5
        let now = utc(2026, 1, 15, 0, 0); // 19:00 the previous local day
        s.set_alarm(now, &zone, 7, 0, None, "x").unwrap();
        assert_eq!(s.list()[0].fire_at, utc(2026, 1, 15, 12, 0));
    }

    #[test]
    fn invalid_alarm_times_are_rejected() {
        let mut s = Scheduler::new(8);
        let z = FixedOffset(0);
        let now = 1_800_000_000;
        assert!(s.set_alarm(now, &z, 24, 0, None, "").is_err());
        assert!(s.set_alarm(now, &z, 7, 60, None, "").is_err());
        assert!(s.set_alarm(now, &z, 7, 0, Some(0), "").is_err());
        assert!(s.set_alarm(now, &z, 7, 0, Some(0b1000_0000), "").is_err());
    }

    #[test]
    fn reminders_must_be_in_the_future() {
        let mut s = Scheduler::new(8);
        let now = utc(2026, 11, 7, 12, 0);
        assert!(s.set_reminder(now, now, "x").is_err());
        assert!(s.set_reminder(now, now - 1, "x").is_err());
        assert!(s.set_reminder(now, now + MAX_AHEAD_SECS + 1, "x").is_err());
        let id = s.set_reminder(now, now + 3600, "call mum").unwrap();
        assert_eq!(s.cancel(id).unwrap().label, "call mum");
        assert_eq!(s.cancel(id), Err(SchedError::NotFound));
    }

    #[test]
    fn nothing_fires_until_the_wall_clock_is_valid() {
        let mut s = Scheduler::new(8);
        let t0 = utc(2026, 11, 7, 12, 0);
        s.set_timer(t0, 10, "x").unwrap();
        let clock = FakeClock::at(t0 + 100);
        clock.valid.set(false);
        assert!(s.take_due(&clock, &FixedOffset(0)).is_empty());
        assert_eq!(s.list().len(), 1, "still pending");
        clock.valid.set(true);
        assert_eq!(s.take_due(&clock, &FixedOffset(0)).len(), 1);
    }

    #[test]
    fn items_found_long_overdue_after_a_restart_are_marked_missed() {
        let mut s = Scheduler::new(8);
        let zone = FixedOffset(0);
        let t0 = utc(2026, 11, 7, 6, 0);
        s.set_alarm(t0, &zone, 7, 0, Some(days::ALL), "wake")
            .unwrap();
        // The device was off all day.
        let clock = FakeClock::at(utc(2026, 11, 7, 15, 0));
        let fired = s.take_due(&clock, &zone);
        assert!(fired[0].missed);
        assert_eq!(fired[0].late_secs, 8 * 3600);
        assert_eq!(
            s.list()[0].fire_at,
            utc(2026, 11, 8, 7, 0),
            "next one is still scheduled"
        );
    }

    #[test]
    fn snooze_creates_a_one_shot_with_the_same_label() {
        let mut s = Scheduler::new(8);
        let zone = FixedOffset(0);
        let t0 = utc(2026, 11, 7, 6, 0);
        s.set_alarm(t0, &zone, 7, 0, None, "wake").unwrap();
        let clock = FakeClock::at(utc(2026, 11, 7, 7, 0));
        let fired = s.take_due(&clock, &zone).remove(0);
        s.snooze(clock.now(), &fired, 540).unwrap();
        assert_eq!(s.list()[0].fire_at, utc(2026, 11, 7, 7, 9));
        assert_eq!(s.list()[0].label, "wake");
        assert!(s.snooze(clock.now(), &fired, 5).is_err());
    }

    #[test]
    fn next_due_in_drives_the_event_loop_sleep() {
        let mut s = Scheduler::new(8);
        assert_eq!(s.next_due_in(100), None);
        let t0 = utc(2026, 11, 7, 12, 0);
        s.set_timer(t0, 30, "a").unwrap();
        s.set_timer(t0, 90, "b").unwrap();
        assert_eq!(s.next_due_in(t0), Some(30));
        assert_eq!(s.next_due_in(t0 + 1000), Some(0));
    }

    #[test]
    fn state_survives_a_restart() {
        let dir = TempDir::new("sched");
        let store = JsonStore::new(dir.0.join("sched.json"), SCHEMA);
        let t0 = utc(2026, 11, 7, 12, 0);
        let mut s = Scheduler::new(8);
        s.set_timer(t0, 600, "pasta").unwrap();
        s.set_reminder(t0, t0 + 7200, "call mum").unwrap();
        s.save(&store).unwrap();
        let restored = Scheduler::load(&store, 8).unwrap();
        assert_eq!(restored.list().len(), 2);
        let mut restored = restored;
        let id = restored.set_timer(t0, 5, "new").unwrap();
        assert_eq!(id, 3, "ids keep counting after a restart");
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
        assert_eq!(
            s.list()[0].fire_at,
            at("2026-03-08T07:00:00-04:00"),
            "07:00 EDT, 11:00Z"
        );
        let clock = FakeClock::at(at("2026-03-08T07:00:00-04:00"));
        s.take_due(&clock, &zone);
        assert_eq!(s.list()[0].fire_at, at("2026-03-09T07:00:00-04:00"));
        // The gap itself: 02:30 does not exist on 2026-03-08; the alarm lands just after the gap.
        let mut s2 = Scheduler::new(8);
        s2.set_alarm(at("2026-03-07T20:00:00-05:00"), &zone, 2, 30, None, "gap")
            .unwrap();
        assert_eq!(s2.list()[0].fire_at, at("2026-03-08T03:30:00-04:00"));
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
        s.stopwatch_start(1_800_000_000).unwrap();
        let json = serde_json::to_string(&s.state).unwrap();
        assert!(json.contains("stopwatch"));
        let back: SchedState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s.state);
        // A stopwatch needs no timer slot, and a full scheduler can still run one.
        let mut full = Scheduler::new(1);
        full.set_timer(1_800_000_000, 60, "").unwrap();
        assert!(full.stopwatch_start(1_800_000_000).is_ok());
    }
}
