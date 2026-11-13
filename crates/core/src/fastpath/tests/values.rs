//! Normalisation and number parsing.

use super::*;

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
