//! The words the core listens for, as data.
//!
//! A [`Language`] holds every word list and phrase pattern the logic acts on: the fast-path grammar and
//! the number, duration and clock words it uses, the yes, no and stop answers, the note commands, the
//! day and month names, the abbreviations that do not end a sentence, and the quit words. The logic
//! (`fastpath`, `session`, `notes`, `llm`, `media`, `assistant`, `commands`) takes a `&'static Language`
//! and holds no word of its own. The language is chosen once, at start-up, as [`DEFAULT`]; today that is
//! English ([`en`]). Nothing here is mutable and nothing is looked up from a global.
//!
//! This module holds words, patterns and the few tiny pure helpers that cannot be separated from them
//! (how an ordinal is written). Logic stays in the modules that use the words.
//!
//! # Not moved yet: full spoken sentences
//!
//! The sentences the host speaks are still written where they are produced. A second language needs
//! these files done next:
//!
//! * `assistant.rs`, `assistant/tools.rs`, `assistant/stopwatch.rs`, `assistant/schedule_api.rs`: replies
//!   to timers, alarms, reminders, the stopwatch, the time and date, and the inbox.
//! * `session.rs`, `session/gate.rs`, `session/gate/rounds.rs`, `session/readback.rs`: confirmations,
//!   the read-back of a held action, and the sentences that end a turn.
//! * `notes.rs`: the replies to note commands.
//! * `media.rs`: station and playback errors.
//! * `inbox/summary.rs`: the status and details sentences.
//! * `app.rs`, `app_loop/turns.rs`, `satellite.rs`, `realtime.rs`, `usage.rs`: error and limit
//!   sentences spoken to the user.
//!
//! Two other English contracts are deliberately left alone: the tool names and argument values the
//! model is offered (`assistant/tools.rs`, `assistant/stopwatch.rs`: "daily", "weekdays", "mon", "lap"),
//! which are an interface to a model, not a spoken language.

use crate::fastpath::Intent;

pub mod en;

/// The language the program runs in. This is the one place it is chosen: start-up passes it to the
/// assistant, the turn settings and the satellites, and the types that have a default read it here.
pub const DEFAULT: &Language = &en::ENGLISH;

/// A noun in two forms, for "1 hour" and "2 hours".
#[derive(Debug, Clone, Copy)]
pub struct Noun {
    /// With a count of one.
    pub one: &'static str,
    /// With any other count.
    pub many: &'static str,
}

/// Number words (REQ-AST-010).
#[derive(Debug, Clone, Copy)]
pub struct Numbers {
    /// Words for 0 to 19. More than one word may give the same value.
    pub small: &'static [(&'static str, u32)],
    /// Words for 20, 30, and so on to 90.
    pub tens: &'static [(&'static str, u32)],
    /// The word that multiplies a digit or "a" by 100.
    pub hundred: &'static str,
    /// The article that stands for one in "a hundred" and "a half".
    pub article_a: &'static str,
    /// The other article that stands for one, before a vowel sound.
    pub article_an: &'static str,
    /// Words refused as the whole of a label, because "set a timer" must not name it "a".
    pub not_a_label: &'static [&'static str],
}

/// Words for lengths of time, spoken and written.
#[derive(Debug, Clone, Copy)]
pub struct DurationWords {
    /// Words that name a unit, with its length in seconds.
    pub units: &'static [(&'static [&'static str], i64)],
    /// "half an hour", as the words said.
    pub half_an_hour: &'static [&'static str],
    /// "and a half", after a number of hours or minutes.
    pub and_a_half: &'static [&'static str],
    /// The word that joins two parts of a length of time, spoken and written.
    pub and: &'static str,
    /// How the hour, minute and second are written.
    pub hour: Noun,
    /// See [`DurationWords::hour`].
    pub minute: Noun,
    /// See [`DurationWords::hour`].
    pub second: Noun,
}

/// Words for the time of day, spoken and written.
#[derive(Debug, Clone, Copy)]
pub struct ClockWords {
    /// Morning marker, as said.
    pub am: &'static str,
    /// Afternoon marker, as said.
    pub pm: &'static str,
    /// Twelve at midday.
    pub noon: &'static str,
    /// Twelve at night.
    pub midnight: &'static str,
    /// Words said for a zero between hour and minute: "seven oh five".
    pub oh: &'static [&'static str],
    /// Words said after an hour for "on the hour".
    pub oclock: &'static [&'static str],
    /// The morning marker as written.
    pub am_label: &'static str,
    /// The afternoon marker as written.
    pub pm_label: &'static str,
}

/// The whole-utterance answers (REQ-SEC-011, REQ-AST-022, REQ-VOI-020). These decide whether a held
/// action runs, so each list is exact.
#[derive(Debug, Clone, Copy)]
pub struct Answers {
    /// Words dropped before an answer is compared.
    pub ignored: &'static [&'static str],
    /// A plain yes.
    pub yes: &'static [&'static str],
    /// A plain no.
    pub no: &'static [&'static str],
    /// A request to stop what is happening.
    pub cancel: &'static [&'static str],
}

/// The note commands (REQ-SEC-017), recognised from the start of the user's own words.
#[derive(Debug, Clone, Copy)]
pub struct NoteWords {
    /// Words that may come first and are skipped.
    pub leading: &'static [&'static str],
    /// Whole utterances that ask to hear the notes.
    pub list: &'static [&'static str],
    /// The start of "remember ...": the words before the note.
    pub remember: &'static [&'static [&'static str]],
    /// The first word of "forget ...".
    pub forget: &'static [&'static str],
    /// What follows "forget" when it means every note.
    pub forget_all: &'static [&'static str],
    /// Words that introduce a note number: "note 3", "number 3".
    pub number_markers: &'static [&'static str],
    /// The word for "the note" in "forget the second note".
    pub note: &'static str,
    /// The article in "the second note".
    pub article: &'static str,
    /// Words for 1 to 10, in order.
    pub numbers: &'static [&'static str],
    /// Words for 1st, 2nd, 3rd, in order.
    pub ordinals: &'static [&'static str],
    /// Words that lead into what to forget, each skipped: "the note about", "about".
    pub subject_leads: &'static [&'static [&'static str]],
    /// Subjects too vague to forget by.
    pub vague: &'static [&'static str],
    /// Words too common to match a note on, lower case (ranking, REQ-MEM-003).
    pub stop_words: &'static [&'static str],
}

/// Names for the calendar, as spoken.
#[derive(Debug, Clone, Copy)]
pub struct Calendar {
    /// Monday first.
    pub weekdays: [&'static str; 7],
    /// January first.
    pub months: [&'static str; 12],
    /// The day being spoken of is the current one.
    pub today: &'static str,
    /// The day being spoken of is the next one.
    pub tomorrow: &'static str,
    /// Goes before a weekday or a date: "on Friday".
    pub on: &'static str,
    /// A day of the month as written: "1st", "22nd".
    pub ordinal: fn(i64) -> String,
}

/// Everything the logic knows about one language.
#[derive(Debug, Clone, Copy)]
pub struct Language {
    /// A short code, for example `en`.
    pub code: &'static str,
    /// The fast-path grammar: one entry per intent, templates in the syntax of `fastpath`. Earlier
    /// entries win ties, so the more specific ones come first.
    pub grammar: &'static [(Intent, &'static [&'static str])],
    /// Polite words dropped from the start of an utterance, each as the words said.
    pub leading_fillers: &'static [&'static [&'static str]],
    /// Polite words dropped from the end of an utterance.
    pub trailing_fillers: &'static [&'static [&'static str]],
    /// Number words.
    pub numbers: Numbers,
    /// Words for lengths of time.
    pub durations: DurationWords,
    /// Words for the time of day.
    pub clock: ClockWords,
    /// Yes, no and stop.
    pub answers: Answers,
    /// The note commands.
    pub notes: NoteWords,
    /// Day and month names.
    pub calendar: Calendar,
    /// Words that end in a full stop without ending the sentence, lower case, without the stop.
    pub abbreviations: &'static [&'static str],
    /// Words that mean "some music" rather than a station name.
    pub generic_music_words: &'static [&'static str],
    /// Words that leave the console.
    pub quit_words: &'static [&'static str],
}

#[cfg(test)]
pub(crate) mod toy;

#[cfg(test)]
mod tests;
