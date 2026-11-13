use super::values::parse_number;
use super::*;
use crate::lang::Language;

const L: &Language = crate::lang::DEFAULT;

fn m(utterance: &str) -> Option<Match> {
    Matcher::builtin(L).matches(utterance)
}

fn intent(utterance: &str) -> Option<&'static str> {
    m(utterance).map(|x| x.intent.name())
}

#[test]
fn the_built_in_grammar_compiles() {
    assert!(!Matcher::builtin(L).intents.is_empty());
    assert!(Matcher::new(L, L.grammar).is_ok());
}

#[test]
fn normalisation_strips_punctuation_and_politeness() {
    assert_eq!(normalize(L, "Hey, could you stop, please?"), vec!["stop"]);
    assert_eq!(
        normalize(L, "Set an alarm for 7 a.m."),
        vec!["set", "an", "alarm", "for", "7", "am"]
    );
    assert_eq!(
        normalize(L, "ten-minute timer"),
        vec!["ten", "minute", "timer"]
    );
}

#[test]
fn numbers_parse_from_digits_and_words() {
    let n = |s: &str| parse_number(L, &normalize(L, s)).map(|(v, _)| v);
    assert_eq!(n("10"), Some(10));
    assert_eq!(n("ten"), Some(10));
    assert_eq!(n("twenty five"), Some(25));
    assert_eq!(n("ninety"), Some(90));
    assert_eq!(n("one hundred"), Some(100));
    assert_eq!(n("banana"), None);
}

#[test]
fn timers_are_recognised_in_many_phrasings() {
    for (u, secs) in [
        ("set a timer for ten minutes", 600),
        ("Set a timer for 10 minutes.", 600),
        ("start a 5 minute timer", 300),
        ("set a ten-minute timer", 600),
        ("timer for 30 seconds", 30),
        ("45 second timer", 45),
        ("set a timer for an hour", 3600),
        ("set a timer for half an hour", 1800),
        ("set a timer for one hour and thirty minutes", 5400),
        ("set a timer for 2 hours 15 minutes", 8100),
        ("set a timer for an hour and a half", 5400),
        ("time me for twenty minutes", 1200),
        ("please set a timer for one minute", 60),
    ] {
        let got = m(u).unwrap_or_else(|| panic!("no match: {u}"));
        assert_eq!(got.intent.name(), "timer.set", "{u}");
        assert_eq!(got.duration("duration"), Some(secs), "{u}");
    }
}

#[test]
fn timer_labels_are_captured() {
    let a = m("set a pasta timer for ten minutes").unwrap();
    assert_eq!(
        (a.text("label"), a.duration("duration")),
        (Some("pasta"), Some(600))
    );
    let b = m("set a timer for ten minutes called laundry").unwrap();
    assert_eq!(
        (b.text("label"), b.duration("duration")),
        (Some("laundry"), Some(600))
    );
}

#[test]
fn timer_management() {
    assert_eq!(intent("cancel the timer"), Some("timer.cancel"));
    assert_eq!(intent("cancel all timers"), Some("timer.cancel"));
    assert_eq!(
        m("cancel the pasta timer").unwrap().text("label"),
        Some("pasta")
    );
    assert_eq!(intent("how much time is left"), Some("timer.query"));
    assert_eq!(
        intent("how much time is left on the pasta timer"),
        Some("timer.query")
    );
}

#[test]
fn timers_can_be_listed_in_many_phrasings() {
    // REQ-TMR-008, REQ-AST-010: listing must not need the model.
    for u in [
        "list timers",
        "list my timers",
        "list all timers",
        "list all of my timers",
        "show my timers",
        "show me the running timers",
        "what timers do I have",
        "what timers are running",
        "what timers are set?",
        "which timers do I have",
        "what are my timers",
        "do I have any timers",
        "are there any timers running",
        "how many timers do I have",
        "please list the timers",
    ] {
        assert_eq!(intent(u), Some("timer.query"), "{u}");
    }
    // Naming a timer still means that timer.
    assert_eq!(
        m("show the pasta timer").unwrap().text("label"),
        Some("pasta")
    );
}

// REQ-TMR-014
#[test]
fn stopwatch_phrasings_are_recognised() {
    for (intent_name, phrases) in [
        (
            "stopwatch.start",
            &[
                "start the stopwatch",
                "start a stopwatch",
                "Start stopwatch.",
                "start the stop watch",
                "start the stop-watch",
                "begin the stopwatch",
                "set up a stopwatch",
                "start timing",
                "start timing me",
                "time me",
                "stopwatch start",
                "please start the stopwatch",
            ][..],
        ),
        (
            "stopwatch.lap",
            &[
                "lap",
                "mark a lap",
                "take a lap",
                "record a lap",
                "next lap",
                "new lap",
                "split",
                "lap time",
                "lap the stopwatch",
            ][..],
        ),
        (
            "stopwatch.stop",
            &[
                "stop the stopwatch",
                "stop stopwatch",
                "end the stopwatch",
                "stop the stop watch",
                "stop timing",
                "stop timing me",
                "stopwatch stop",
            ][..],
        ),
        (
            "stopwatch.read",
            &[
                "how long on the stopwatch",
                "how long has the stopwatch been running",
                "what's the stopwatch at",
                "what is the stopwatch reading",
                "check the stopwatch",
                "read the stopwatch",
                "how much time is on the stopwatch",
                "how much time has passed on the stopwatch",
                "stopwatch time",
            ][..],
        ),
        (
            "stopwatch.reset",
            &[
                "reset the stopwatch",
                "clear the stopwatch",
                "cancel the stopwatch",
                "reset stopwatch",
            ][..],
        ),
    ] {
        for u in phrases {
            assert_eq!(intent(u), Some(intent_name), "{u}");
        }
    }
}

// REQ-TMR-014: the stopwatch phrases must not take over timers, the bare stop, or ordinary requests.
#[test]
fn stopwatch_phrasings_do_not_steal_timers_or_stop() {
    for (u, want) in [
        ("stop", Some("stop")),
        ("stop the timer", Some("timer.cancel")),
        ("cancel all timers", Some("timer.cancel")),
        ("set a timer for ten minutes", Some("timer.set")),
        ("time me for ten minutes", Some("timer.set")),
        ("start a timer for 5 minutes", Some("timer.set")),
        ("how much time is left on the timer", Some("timer.query")),
        ("what time is it", Some("time.query")),
        ("cancel the alarm", Some("alarm.cancel")),
    ] {
        assert_eq!(intent(u), want, "{u}");
    }
    for u in [
        "stop watching television",
        "start the dishwasher",
        "what's a stopwatch",
        "lap swimming is good exercise",
        "how long is the lap of a marathon",
        "pause the stopwatch",
    ] {
        let got = intent(u);
        assert!(
            got.is_none_or(|i| !i.starts_with("stopwatch.")),
            "{u} -> {got:?}"
        );
    }
}

#[test]
fn alarms() {
    let a = m("set an alarm for 7 am").unwrap();
    assert_eq!(a.intent.name(), "alarm.set");
    assert_eq!(
        a.time("time"),
        Some(Time {
            hour: 7,
            minute: 0,
            meridiem: Some(Meridiem::Am)
        })
    );
    let b = m("wake me up at seven thirty").unwrap();
    assert_eq!(
        b.time("time"),
        Some(Time {
            hour: 7,
            minute: 30,
            meridiem: None
        })
    );
    assert_eq!(
        b.time("time").unwrap().candidates_24h(),
        vec![(7, 30), (19, 30)]
    );
    let c = m("set an alarm for 6:45 pm").unwrap();
    assert_eq!(c.time("time").unwrap().candidates_24h(), vec![(18, 45)]);
    assert_eq!(
        intent("set a weekday alarm for 6:30 am"),
        Some("alarm.set_weekdays")
    );
    assert_eq!(
        intent("set a daily alarm for seven"),
        Some("alarm.set_daily")
    );
    assert_eq!(
        intent("set a weekend alarm for 9 am"),
        Some("alarm.set_weekend")
    );
    assert_eq!(intent("cancel my alarm"), Some("alarm.cancel"));
    assert_eq!(
        m("set an alarm for noon")
            .unwrap()
            .time("time")
            .unwrap()
            .candidates_24h(),
        vec![(12, 0)]
    );
    assert_eq!(
        m("set an alarm for midnight")
            .unwrap()
            .time("time")
            .unwrap()
            .candidates_24h(),
        vec![(0, 0)]
    );
    assert_eq!(
        m("set an alarm for 7 o'clock")
            .unwrap()
            .time("time")
            .unwrap()
            .minute,
        0
    );
}

#[test]
fn reminders() {
    let a = m("remind me to call mum in ten minutes").unwrap();
    assert_eq!(a.intent.name(), "reminder.set_in");
    assert_eq!(
        (a.text("text"), a.duration("duration")),
        (Some("call mum"), Some(600))
    );
    let b = m("remind me in an hour to take the bins out").unwrap();
    assert_eq!(
        (b.text("text"), b.duration("duration")),
        (Some("take the bins out"), Some(3600))
    );
    let c = m("remind me at 3 pm to pick up the kids").unwrap();
    assert_eq!(c.intent.name(), "reminder.set_at");
    assert_eq!(c.text("text"), Some("pick up the kids"));
}

#[test]
fn stop_volume_and_media_controls() {
    assert_eq!(intent("stop"), Some("stop"));
    assert_eq!(intent("Never mind."), Some("stop"));
    assert_eq!(intent("volume up"), Some("volume.up"));
    assert_eq!(intent("turn it down"), Some("volume.down"));
    assert_eq!(
        m("set the volume to 40 percent").unwrap().number("level"),
        Some(40)
    );
    assert_eq!(m("volume sixty").unwrap().number("level"), Some(60));
    assert_eq!(intent("pause the music"), Some("media.pause"));
    assert_eq!(intent("stop the music"), Some("media.pause"));
    assert_eq!(intent("resume"), Some("media.resume"));
    assert_eq!(intent("skip this song"), Some("media.next"));
    assert_eq!(intent("play some jazz"), Some("media.play"));
    assert_eq!(intent("put on the bbc world service"), Some("media.play"));
    // "play the music" is a resume, not a station called "music".
    assert_eq!(intent("play the music"), Some("media.resume"));
}

#[test]
fn inbox_commands_are_recognised() {
    assert_eq!(intent("what are my agents up to"), Some("inbox.status"));
    assert_eq!(intent("any updates"), Some("inbox.status"));
    assert_eq!(intent("give me the details"), Some("inbox.details"));
    assert_eq!(intent("tell me more"), Some("inbox.details"));
    assert_eq!(intent("thanks I'll handle it"), Some("inbox.ack"));
    assert_eq!(intent("I've got it"), Some("inbox.ack"));
    assert_eq!(intent("dismiss it"), Some("inbox.dismiss"));
    assert_eq!(intent("dismiss"), Some("inbox.dismiss"));
    let s = m("snooze for twenty minutes").unwrap();
    assert_eq!(
        (s.intent.name(), s.duration("duration")),
        ("inbox.snooze", Some(1200))
    );
    assert_eq!(intent("not now"), Some("inbox.snooze"));
    let i = m("tell it to also add unit tests").unwrap();
    assert_eq!(
        (i.intent.name(), i.text("text")),
        ("inbox.instruct", Some("also add unit tests"))
    );
    let j = m("keep going and fix the failing test").unwrap();
    assert_eq!(j.text("text"), Some("fix the failing test"));
    assert_eq!(intent("keep going"), Some("inbox.instruct"));
}

#[test]
fn time_and_date_are_answered_locally() {
    assert_eq!(intent("what time is it"), Some("time.query"));
    assert_eq!(intent("What's the date today?"), Some("date.query"));
}

#[test]
fn anything_else_falls_through_to_the_llm() {
    for u in [
        "what's the capital of Australia",
        "tell me a joke",
        "set a timer",
        "set an alarm",
        "remind me",
        "how tall is mount everest",
        "stop being so annoying and tell me a story about dragons",
        "",
        "   ",
    ] {
        assert_eq!(intent(u), None, "{u:?} should fall through");
    }
}

#[test]
fn malformed_templates_are_rejected_not_ignored() {
    for bad in [
        "(unclosed",
        "unclosed)",
        "[oops",
        "{slot",
        "{}",
        "()",
        "[a|b]",
    ] {
        assert!(Matcher::new(L, &[("x", &[bad])]).is_err(), "{bad}");
    }
}

#[test]
fn deeply_nested_templates_are_refused_instead_of_overflowing_the_stack() {
    // Templates are compiled in today, but packs will bring them from outside (REQ-EXT-002).
    let nested = format!("{}a{}", "(".repeat(100_000), ")".repeat(100_000));
    assert!(Matcher::new(L, &[("x", &[nested.as_str()])]).is_err());
    let optional = format!("{}a{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(Matcher::new(L, &[("x", &[optional.as_str()])]).is_err());
    // Ordinary nesting still works.
    assert!(Matcher::new(L, &[("x", &["(a (b [c]) | d) e"])]).is_ok());
}

#[test]
fn explosive_templates_are_refused() {
    let t = "[a] [b] [c] [d] [e] [f] [g] [h] [i] [j] x";
    assert!(Matcher::new(L, &[("x", &[t])]).is_err());
}

#[test]
fn pathological_input_cannot_stall_the_matcher() {
    let matcher = Matcher::new(
        L,
        &[(
            "greedy",
            &["{a:text} {b:text} {c:text} {d:text} {e:text} {f:text} end"],
        )],
    )
    .unwrap();
    let long = "word ".repeat(200);
    let start = std::time::Instant::now();
    assert!(matcher.matches(&long).is_none());
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn matching_is_fast_enough_for_the_budget() {
    // REQ-NFR-111: at most 20 ms per utterance on the reference CPU. Check a generous bound in
    // debug builds on a fast host so a regression to something quadratic is caught.
    let matcher = Matcher::builtin(L);
    let start = std::time::Instant::now();
    for _ in 0..200 {
        let _ = matcher.matches("set a timer for ten minutes");
        let _ = matcher.matches("what's the capital of australia please");
    }
    let per = start.elapsed() / 400;
    assert!(
        per < std::time::Duration::from_millis(5),
        "per match {per:?}"
    );
}
