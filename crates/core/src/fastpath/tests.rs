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

mod commands;
mod grammar;
mod values;
