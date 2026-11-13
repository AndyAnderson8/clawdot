//! Result types: clock times, slot values and recognised commands.

use super::intent::Intent;

/// Morning or afternoon marker spoken with a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meridiem {
    /// "am"
    Am,
    /// "pm"
    Pm,
}

/// A spoken clock time, before it is resolved against "now".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Time {
    /// Hour as spoken (1 to 12, or 0 to 24 for 24-hour speech).
    pub hour: u8,
    /// Minute, 0 to 59.
    pub minute: u8,
    /// "am" or "pm" if said.
    pub meridiem: Option<Meridiem>,
}

impl Time {
    /// The possible 24-hour `(hour, minute)` readings. A bare "seven" is ambiguous (07:00 or 19:00); the
    /// caller picks whichever comes next.
    #[must_use]
    pub fn candidates_24h(&self) -> Vec<(u8, u8)> {
        let m = self.minute;
        match self.meridiem {
            Some(Meridiem::Am) => vec![(self.hour % 12, m)],
            Some(Meridiem::Pm) => vec![(self.hour % 12 + 12, m)],
            None if self.hour == 0 || self.hour >= 13 => vec![(self.hour % 24, m)],
            None => vec![(self.hour % 12, m), (self.hour % 12 + 12, m)],
        }
    }
}

/// A value captured by a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotValue {
    /// Free text.
    Text(String),
    /// A whole number.
    Number(i64),
    /// A length of time in seconds.
    Duration(i64),
    /// A clock time.
    Time(Time),
}

/// A recognised command. `I` is the intent type: [`Intent`] for the built-in grammar, anything copyable
/// for a custom one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match<I = Intent> {
    /// What was asked for.
    pub intent: I,
    /// Captured slots in template order.
    pub slots: Vec<(String, SlotValue)>,
}

impl<I> Match<I> {
    /// A slot by name.
    #[must_use]
    pub fn slot(&self, name: &str) -> Option<&SlotValue> {
        self.slots.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
    /// A duration slot, in seconds.
    #[must_use]
    pub fn duration(&self, name: &str) -> Option<i64> {
        match self.slot(name)? {
            SlotValue::Duration(d) => Some(*d),
            _ => None,
        }
    }
    /// A number slot.
    #[must_use]
    pub fn number(&self, name: &str) -> Option<i64> {
        match self.slot(name)? {
            SlotValue::Number(n) => Some(*n),
            _ => None,
        }
    }
    /// A text slot.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.slot(name)? {
            SlotValue::Text(t) => Some(t),
            _ => None,
        }
    }
    /// A time slot.
    #[must_use]
    pub fn time(&self, name: &str) -> Option<Time> {
        match self.slot(name)? {
            SlotValue::Time(t) => Some(*t),
            _ => None,
        }
    }
}
