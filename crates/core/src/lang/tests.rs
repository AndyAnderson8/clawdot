use super::toy::TOY;
use super::*;
use crate::assistant::{format_clock, format_duration};
use crate::fastpath::{Intent, Matcher, Meridiem, SlotValue, Time};
use crate::llm::SentenceSplitter;
use crate::media::{Action, Player, Station};
use crate::notes::{self, Cmd};

const EN: &Language = DEFAULT;
const XX: &Language = &TOY;

#[test]
fn every_intent_has_a_template_in_the_english_grammar() {
    for intent in Intent::ALL {
        let entries: Vec<_> = EN.grammar.iter().filter(|(i, _)| *i == intent).collect();
        assert_eq!(
            entries.len(),
            1,
            "{} must have exactly one grammar entry",
            intent.name()
        );
        assert!(
            !entries[0].1.is_empty(),
            "{} has no template",
            intent.name()
        );
    }
    for (intent, _) in EN.grammar {
        assert!(
            Intent::ALL.contains(intent),
            "{} is not listed",
            intent.name()
        );
    }
}

#[test]
fn every_english_template_compiles() {
    for (intent, templates) in EN.grammar {
        for t in *templates {
            assert!(
                Matcher::new(EN, &[(*intent, &[*t][..])]).is_ok(),
                "{}: {t}",
                intent.name()
            );
        }
    }
    assert!(Matcher::new(EN, EN.grammar).is_ok());
}

#[test]
fn the_intent_names_are_unique_and_dotted() {
    let mut seen = std::collections::HashSet::new();
    for intent in Intent::ALL {
        assert!(seen.insert(intent.name()), "{} twice", intent.name());
    }
    assert_eq!(Intent::ALL.len(), seen.len());
}

// The tests below run the logic with a language that shares no word with English. If one of the files
// that were moved over regains a word list of its own, the matching test fails.

#[test]
fn yes_no_and_stop_come_from_the_language() {
    use crate::session::{is_cancel, is_no, is_yes};
    assert!(is_yes(XX, "Ja!") && is_yes(XX, "bitte ja"));
    assert!(!is_yes(XX, "yes") && !is_yes(XX, "okay"));
    assert!(is_no(XX, "nein") && !is_no(XX, "no") && !is_no(XX, "stop"));
    assert!(is_cancel(XX, "abbrechen") && !is_cancel(XX, "stop") && !is_cancel(XX, "cancel"));
}

#[test]
fn the_quit_words_come_from_the_language() {
    use crate::commands::is_quit;
    assert!(is_quit(XX, "Ende.") && !is_quit(XX, "quit") && !is_quit(XX, "exit"));
}

#[test]
fn the_fast_path_reads_its_numbers_durations_and_clock_from_the_language() {
    let m = Matcher::builtin(XX);
    let timer = |u: &str| m.matches(u);
    let got = timer("bitte timer drei minuten").unwrap();
    assert_eq!(got.intent, Intent::TimerSet);
    assert_eq!(got.duration("duration"), Some(180));
    assert_eq!(
        timer("timer eine halbe stunde")
            .unwrap()
            .duration("duration"),
        Some(1800)
    );
    assert_eq!(
        timer("timer eine stunde und eine halbe")
            .unwrap()
            .duration("duration"),
        Some(5400)
    );
    assert_eq!(
        timer("timer zwanzig drei sekunden danke")
            .unwrap()
            .duration("duration"),
        Some(23)
    );
    assert_eq!(
        timer("timer drei minuten tee").unwrap().text("label"),
        Some("tee")
    );
    // A lone article is not a label, in the language's own articles.
    assert!(timer("timer drei minuten der").is_none());
    // English words mean nothing here.
    assert!(timer("timer three minutes").is_none());
    assert!(timer("set a timer for three minutes").is_none());
    assert!(timer("please timer drei minuten").is_none());

    let time = |u: &str| m.matches(u).unwrap().time("time");
    assert_eq!(
        time("wecker mittag"),
        Some(Time {
            hour: 12,
            minute: 0,
            meridiem: Some(Meridiem::Pm)
        })
    );
    assert_eq!(
        time("wecker sieben nm"),
        Some(Time {
            hour: 7,
            minute: 0,
            meridiem: Some(Meridiem::Pm)
        })
    );
    assert_eq!(
        m.matches("lautstaerke zwanzig").unwrap().slot("level"),
        Some(&SlotValue::Number(20))
    );
    assert_eq!(m.matches("halt").map(|x| x.intent), Some(Intent::Stop));
    assert!(m.matches("stop").is_none());
}

#[test]
fn the_note_commands_come_from_the_language() {
    let p = |u: &str| notes::parse(XX, u);
    assert_eq!(
        p("bitte merke dass das Licht kaputt ist"),
        Some(Cmd::Remember("das Licht kaputt ist".into()))
    );
    assert_eq!(p("zeige meine notizen"), Some(Cmd::List));
    assert_eq!(p("vergiss alles"), Some(Cmd::ForgetAll));
    assert_eq!(p("vergiss notiz zwei"), Some(Cmd::ForgetNth(2)));
    assert_eq!(p("vergiss die erste notiz"), Some(Cmd::ForgetNth(1)));
    assert_eq!(
        p("vergiss ueber den drucker"),
        Some(Cmd::ForgetText("den drucker".into()))
    );
    assert_eq!(p("vergiss ueber es"), None);
    for english in [
        "remember that my printer is called Pixel",
        "what are my notes",
        "forget everything",
        "forget note 2",
    ] {
        assert_eq!(p(english), None, "{english}");
    }
}

#[test]
fn the_sentence_abbreviations_come_from_the_language() {
    let split = |lang: &'static Language| {
        let mut s = SentenceSplitter::new(lang);
        s.push("Siehe bzw. das Bild. Danke")
    };
    assert_eq!(split(XX), vec!["Siehe bzw. das Bild."]);
    // English does not know `bzw`, so it cuts there.
    assert_eq!(split(EN), vec!["Siehe bzw.", "das Bild."]);
    let mut s = SentenceSplitter::new(XX);
    assert_eq!(s.push("Dr. Meier kommt. "), vec!["Dr.", "Meier kommt."]);
}

#[test]
fn the_words_for_some_music_come_from_the_language() {
    let stations = || {
        vec![
            Station {
                name: "Rock".into(),
                url: "http://radio.example/rock".into(),
            },
            Station {
                name: "Jazz".into(),
                url: "http://radio.example/jazz".into(),
            },
        ]
    };
    let title = |lang: &'static Language, q: &str| {
        Player::new(lang, stations()).play(q).ok().map(|a| match a {
            Action::Play { title, .. } => title,
            other => format!("{other:?}"),
        })
    };
    assert_eq!(title(XX, "irgendwas").as_deref(), Some("Rock"));
    assert_eq!(
        title(XX, "some music"),
        None,
        "English filler is a station name here"
    );
    assert_eq!(title(EN, "some music").as_deref(), Some("Rock"));
    assert_eq!(title(EN, "irgendwas"), None);
}

#[test]
fn spoken_forms_of_time_and_dates_come_from_the_language() {
    assert_eq!(
        format_duration(XX, 3661),
        "1 Stunde, 1 Minute und 1 Sekunde"
    );
    assert_eq!(format_duration(XX, 45), "45 Sekunden");
    assert_eq!(format_clock(XX, 15, 30), "3:30 nachm.");
    assert_eq!(format_clock(XX, 0, 0), "12 vorm.");
    assert_eq!((XX.calendar.ordinal)(22), "22.");
    assert_eq!((EN.calendar.ordinal)(22), "22nd");
}

#[test]
fn the_words_too_common_to_rank_a_note_by_come_from_the_language() {
    let search = |lang: &'static Language| {
        let s = notes::NoteStore::in_memory(lang, true);
        s.add("der drucker ist kaputt", 1).unwrap();
        s.search("der ist", 5).len()
    };
    assert_eq!(
        search(XX),
        0,
        "both words are stop words in the toy language"
    );
    assert_eq!(search(EN), 1, "they are ordinary words in English");
}
