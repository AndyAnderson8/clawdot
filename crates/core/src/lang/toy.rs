//! A made-up language for tests. Every word differs from English, so a test that passes with it shows the
//! logic reads the words from the [`Language`] and holds none of its own.

use super::{Answers, Calendar, ClockWords, DurationWords, Language, NoteWords, Noun, Numbers};
use crate::fastpath::Intent;

fn ordinal(n: i64) -> String {
    format!("{n}.")
}

/// The test language, German-flavoured so it is easy to read.
pub const TOY: Language = Language {
    code: "xx",
    grammar: &[
        (Intent::Stop, &["halt", "genug"]),
        (
            Intent::TimerSet,
            &["timer {duration}", "timer {duration} {label:text}"],
        ),
        (Intent::AlarmSet, &["wecker {time}"]),
        (Intent::VolumeSet, &["lautstaerke {level:number}"]),
    ],
    leading_fillers: &[&["bitte"], &["koennten", "sie"]],
    trailing_fillers: &[&["danke"]],
    numbers: Numbers {
        small: &[
            ("null", 0),
            ("eins", 1),
            ("zwei", 2),
            ("drei", 3),
            ("sieben", 7),
            ("zehn", 10),
        ],
        tens: &[("zwanzig", 20), ("dreissig", 30)],
        hundred: "hundert",
        article_a: "ein",
        article_an: "eine",
        not_a_label: &["der", "die"],
    },
    durations: DurationWords {
        units: &[
            (&["sekunde", "sekunden"], 1),
            (&["minute", "minuten"], 60),
            (&["stunde", "stunden"], 3600),
        ],
        half_an_hour: &["eine", "halbe", "stunde"],
        and_a_half: &["und", "eine", "halbe"],
        and: "und",
        hour: Noun {
            one: "Stunde",
            many: "Stunden",
        },
        minute: Noun {
            one: "Minute",
            many: "Minuten",
        },
        second: Noun {
            one: "Sekunde",
            many: "Sekunden",
        },
    },
    clock: ClockWords {
        am: "vm",
        pm: "nm",
        noon: "mittag",
        midnight: "mitternacht",
        oh: &["null"],
        oclock: &["uhr"],
        am_label: "vorm.",
        pm_label: "nachm.",
    },
    answers: Answers {
        ignored: &["bitte"],
        yes: &["ja", "jawohl"],
        no: &["nein", "nee"],
        cancel: &["halt", "abbrechen"],
    },
    notes: NoteWords {
        leading: &["bitte"],
        list: &["zeige meine notizen"],
        remember: &[&["merke", "dass"], &["merke"]],
        forget: &["vergiss"],
        forget_all: &["alles"],
        number_markers: &["notiz"],
        note: "notiz",
        article: "die",
        numbers: &["eins", "zwei", "drei"],
        ordinals: &["erste", "zweite", "dritte"],
        subject_leads: &[&["die", "notiz", "ueber"], &["ueber"]],
        vague: &["", "es"],
        stop_words: &["der", "die", "das", "und", "ist"],
    },
    calendar: Calendar {
        weekdays: [
            "Montag",
            "Dienstag",
            "Mittwoch",
            "Donnerstag",
            "Freitag",
            "Samstag",
            "Sonntag",
        ],
        months: [
            "Januar",
            "Februar",
            "Maerz",
            "April",
            "Mai",
            "Juni",
            "Juli",
            "August",
            "September",
            "Oktober",
            "November",
            "Dezember",
        ],
        today: "heute",
        tomorrow: "morgen",
        on: "am",
        ordinal,
    },
    abbreviations: &["bzw"],
    generic_music_words: &["irgendwas"],
    quit_words: &["ende"],
};
