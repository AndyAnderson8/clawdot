//! Timers, alarms, and reminders (REQ-TMR-001..008).
//!
//! Two kinds of time are kept apart here. A *duration* (a countdown timer, a snooze, "remind me in ten
//! minutes") runs on [`Clock::monotonic_secs`], which never goes backwards and does not notice a step of the
//! wall clock, so network time arriving mid-countdown cannot make it ring early, late or "missed". A point on
//! the *calendar* (an alarm, a reminder at a date and time) is a UTC unix second compared with
//! [`Clock::now`], so it follows a corrected clock to the true 07:30. A monotonic reading means nothing after
//! a restart, so a countdown is saved as the seconds it still has left plus the wall time of the save, and the
//! downtime is taken off it at load.
//!
//! All calendar deadlines are UTC unix seconds. Alarm recurrence is expressed in *local* wall time through the
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
    /// Current UTC unix time. It can step in either direction when the clock is set; use it for points on
    /// the calendar and never to measure how long something has lasted.
    fn now(&self) -> Ts;
    /// Whole seconds since an arbitrary origin (at least 1, so 0 can mean "never"). It never goes backwards and
    /// is not affected by a step of [`Clock::now`]; use it to measure durations, and compare only readings
    /// taken in the same process, because it means nothing after a restart. Required rather than defaulted to
    /// `now`, so no implementation can silently give a duration the wall clock's flaw.
    fn monotonic_secs(&self) -> i64;
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
    /// Seconds since the first reading in this process, from [`std::time::Instant`]. On Linux that is
    /// `CLOCK_MONOTONIC`, which stops while the machine is suspended, so a timer on a laptop loses the time it
    /// slept. The devices this runs on never suspend, so this is accepted; the fix (`CLOCK_BOOTTIME`) needs a
    /// `libc` call or a new dependency and is not worth it for them.
    fn monotonic_secs(&self) -> i64 {
        static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let secs = ORIGIN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs();
        i64::try_from(secs)
            .unwrap_or(i64::MAX - 1)
            .saturating_add(1)
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
    /// Next firing instant on the calendar. For a countdown (see [`Item::is_countdown`]) it is only what was
    /// true when it was created or last saved; read it through [`Item::due_wall`].
    pub fire_at: Ts,
    /// Recurrence, for alarms.
    pub repeat: Option<Repeat>,
    /// When it was created.
    pub created_at: Ts,
    /// Persisted form of a countdown: the seconds it still had left when the file was written (negative
    /// when it was already late). Present in a file only; consumed at load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remaining_secs: Option<i64>,
    /// Persisted form of a countdown: the wall time of that save, if the clock was valid then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saved_at: Option<Ts>,
    /// The monotonic reading at which a countdown comes due. Never written to disk.
    #[serde(skip)]
    due_mono: Option<i64>,
}

impl Item {
    fn new(kind: Kind, label: &str, fire_at: Ts, repeat: Option<Repeat>, created_at: Ts) -> Self {
        Self {
            id: 0,
            kind,
            label: label.to_owned(),
            fire_at,
            repeat,
            created_at,
            remaining_secs: None,
            saved_at: None,
            due_mono: None,
        }
    }

    fn countdown(kind: Kind, label: &str, clock: &dyn Clock, secs: i64) -> Self {
        let now = clock.now();
        let mut item = Self::new(kind, label, now.saturating_add(secs), None, now);
        item.due_mono = Some(clock.monotonic_secs().saturating_add(secs));
        item
    }

    /// A duration that runs on the monotonic clock (a timer, a snooze, a reminder after a delay) rather than
    /// a point on the calendar.
    #[must_use]
    pub fn is_countdown(&self) -> bool {
        self.due_mono.is_some()
    }

    /// Seconds until it comes due, never negative.
    #[must_use]
    pub fn secs_left(&self, clock: &dyn Clock) -> i64 {
        self.signed_left(clock).max(0)
    }

    /// Seconds until it comes due; negative once it is late.
    fn signed_left(&self, clock: &dyn Clock) -> i64 {
        match self.due_mono {
            Some(m) => m.saturating_sub(clock.monotonic_secs()),
            None => self.fire_at.saturating_sub(clock.now()),
        }
    }

    /// When it comes due as a wall-clock instant, worked out now, so what a person is shown stays right after
    /// the wall clock was set.
    #[must_use]
    pub fn due_wall(&self, clock: &dyn Clock) -> Ts {
        match self.due_mono {
            Some(_) => clock.now().saturating_add(self.signed_left(clock)),
            None => self.fire_at,
        }
    }
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

    /// Restore from a store and put the countdowns back on the monotonic clock.
    ///
    /// A countdown is in the file as the seconds it had left and the wall time of the save. The time the
    /// program was off is taken off with the wall clock (REQ-TMR-003), as far as the wall clock can be
    /// trusted: a clock that is not valid, or is behind the save, takes off nothing, never a negative amount.
    /// A countdown that ran out while the program was off comes back already late, so it is announced as
    /// missed when it is more than [`MISSED_GRACE_SECS`] late. A file from before this form (a timer with only
    /// an absolute `fire_at`) is read as the wall clock then says: `fire_at - now` seconds left.
    ///
    /// # Errors
    /// Storage errors (corrupt or newer-schema files are reported, never silently dropped).
    pub fn load(
        store: &JsonStore,
        max_items: usize,
        clock: &dyn Clock,
    ) -> Result<Self, StoreError> {
        let mut state: SchedState = store.load()?;
        let (wall, mono, valid) = (
            clock.now(),
            clock.monotonic_secs(),
            clock.wall_clock_valid(),
        );
        for item in &mut state.items {
            let left = match (item.remaining_secs.take(), item.saved_at.take()) {
                (Some(remaining), saved) => {
                    let off = match saved {
                        Some(at) if valid => wall.saturating_sub(at).max(0),
                        _ => 0,
                    };
                    remaining.saturating_sub(off)
                }
                // Written before countdowns were kept apart: only timers are durations.
                (None, _) if item.kind == Kind::Timer => {
                    let from = if valid { wall } else { item.created_at };
                    item.fire_at.saturating_sub(from).min(MAX_TIMER_SECS)
                }
                (None, _) => continue,
            };
            item.due_mono = Some(mono.saturating_add(left));
        }
        Ok(Self { state, max_items })
    }

    /// Persist atomically.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore, clock: &dyn Clock) -> Result<(), StoreError> {
        store.save(&self.persisted(clock))
    }

    /// The bytes [`Scheduler::save`] would write, for handing to the background writer.
    ///
    /// # Errors
    /// A state that cannot be serialised.
    pub fn encode(&self, store: &JsonStore, clock: &dyn Clock) -> Result<Vec<u8>, StoreError> {
        store.encode(&self.persisted(clock))
    }

    /// The state as it goes to disk. The seconds a countdown has left are worked out afresh at every save,
    /// because the file is rewritten only when the set of items changes and an old figure would be wrong.
    /// `fire_at` is refreshed too, so a build from before this form still reads a sensible deadline.
    fn persisted(&self, clock: &dyn Clock) -> SchedState {
        let mut state = self.state.clone();
        let wall = clock.now();
        let valid = clock.wall_clock_valid();
        for item in state.items.iter_mut().filter(|i| i.due_mono.is_some()) {
            let left = item.signed_left(clock);
            item.remaining_secs = Some(left);
            item.saved_at = valid.then_some(wall);
            item.fire_at = wall.saturating_add(left);
        }
        state
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
    pub fn set_timer(
        &mut self,
        clock: &dyn Clock,
        secs: i64,
        label: &str,
    ) -> Result<u64, SchedError> {
        if !(1..=MAX_TIMER_SECS).contains(&secs) {
            return Err(SchedError::InvalidTime);
        }
        self.insert(Item::countdown(Kind::Timer, label, clock, secs))
    }

    /// A reminder after a delay ("remind me in ten minutes"): a duration, so it runs on the monotonic clock.
    ///
    /// # Errors
    /// [`SchedError::InvalidTime`] when `secs` is not in the future or is over a year away;
    /// [`SchedError::TooMany`].
    pub fn set_reminder_in(
        &mut self,
        clock: &dyn Clock,
        secs: i64,
        text: &str,
    ) -> Result<u64, SchedError> {
        if !(1..=MAX_AHEAD_SECS).contains(&secs) {
            return Err(SchedError::InvalidTime);
        }
        self.insert(Item::countdown(Kind::Reminder, text, clock, secs))
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
        self.insert(Item::new(
            Kind::Alarm,
            label,
            fire_at,
            days.map(|d| Repeat {
                days: d,
                hour,
                minute,
            }),
            now,
        ))
    }

    /// A reminder at the instant `at`.
    ///
    /// # Errors
    /// [`SchedError::InvalidTime`] when `at` is not in the future or is over a year away.
    pub fn set_reminder(&mut self, now: Ts, at: Ts, text: &str) -> Result<u64, SchedError> {
        if at <= now || at - now > MAX_AHEAD_SECS {
            return Err(SchedError::InvalidTime);
        }
        self.insert(Item::new(Kind::Reminder, text, at, None, now))
    }

    /// Ring again in `secs`: a one-shot carrying the label of a fired item. A duration, so it runs on the
    /// monotonic clock.
    ///
    /// # Errors
    /// [`SchedError::InvalidTime`] outside 1 minute to 1 hour; [`SchedError::TooMany`].
    pub fn snooze(
        &mut self,
        clock: &dyn Clock,
        fired: &Fired,
        secs: i64,
    ) -> Result<u64, SchedError> {
        if !(60..=3600).contains(&secs) {
            return Err(SchedError::InvalidTime);
        }
        self.insert(Item::countdown(fired.kind, &fired.label, clock, secs))
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
    pub fn list(&self, clock: &dyn Clock) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.state.items.iter().collect();
        v.sort_by_key(|i| (i.due_wall(clock), i.id));
        v
    }

    /// Seconds left on a timer (or time until any item).
    ///
    /// # Errors
    /// [`SchedError::NotFound`].
    pub fn remaining(&self, id: u64, clock: &dyn Clock) -> Result<i64, SchedError> {
        self.state
            .items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.secs_left(clock))
            .ok_or(SchedError::NotFound)
    }

    /// Collect everything that has come due. Calendar items wait until the wall clock is valid
    /// (REQ-TMR-006); a countdown does not depend on the wall clock at all, so it comes due on a device that
    /// has never had the time. Repeating alarms are rescheduled; everything else is removed.
    pub fn take_due(&mut self, clock: &dyn Clock, zone: &dyn Zone) -> Vec<Fired> {
        let wall_ok = clock.wall_clock_valid();
        let now = clock.now();
        let mut fired = Vec::new();
        let mut keep = Vec::with_capacity(self.state.items.len());
        for mut item in std::mem::take(&mut self.state.items) {
            if !wall_ok && !item.is_countdown() {
                keep.push(item);
                continue;
            }
            let late = -item.signed_left(clock);
            if late < 0 {
                keep.push(item);
                continue;
            }
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

/// A clock for tests whose wall and monotonic readings move independently, so a test can step one and not
/// the other, the way network time arriving does.
#[cfg(test)]
pub(crate) mod testclock {
    use super::{Clock, Ts};
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

    /// Wall time and monotonic time are separate. [`StepClock::advance`] moves both (time passing),
    /// [`StepClock::step_wall`] moves only the wall clock (it was set), and the monotonic reading starts well away
    /// from the wall one so mixing them up shows.
    #[derive(Debug)]
    pub(crate) struct StepClock {
        wall: AtomicI64,
        mono: AtomicI64,
        unset: AtomicBool,
    }

    impl StepClock {
        /// Wall time `wall`; the monotonic reading starts at 1000.
        pub(crate) fn at(wall: Ts) -> Self {
            Self::with_mono(wall, 1000)
        }
        /// As [`StepClock::at`], with a chosen monotonic reading (a restarted process has a new origin).
        pub(crate) fn with_mono(wall: Ts, mono: i64) -> Self {
            Self {
                wall: AtomicI64::new(wall),
                mono: AtomicI64::new(mono),
                unset: AtomicBool::new(false),
            }
        }
        /// `secs` pass on both clocks.
        pub(crate) fn advance(&self, secs: i64) {
            self.wall.fetch_add(secs, Ordering::SeqCst);
            self.mono.fetch_add(secs, Ordering::SeqCst);
        }
        /// The wall clock is set `secs` forward (or back, when negative); no time passes.
        pub(crate) fn step_wall(&self, secs: i64) {
            self.wall.fetch_add(secs, Ordering::SeqCst);
        }
        /// Put the wall clock at `t` without any time passing.
        pub(crate) fn set_wall(&self, t: Ts) {
            self.wall.store(t, Ordering::SeqCst);
        }
        /// Make the wall clock report that it was never set (or that it has been).
        pub(crate) fn set_unset(&self, unset: bool) {
            self.unset.store(unset, Ordering::SeqCst);
        }
    }

    impl Clock for StepClock {
        fn now(&self) -> Ts {
            self.wall.load(Ordering::SeqCst)
        }
        fn monotonic_secs(&self) -> i64 {
            self.mono.load(Ordering::SeqCst)
        }
        fn wall_clock_valid(&self) -> bool {
            !self.unset.load(Ordering::SeqCst) && self.now() >= super::CLOCK_VALID_AFTER
        }
    }
}

#[cfg(test)]
mod tests;
