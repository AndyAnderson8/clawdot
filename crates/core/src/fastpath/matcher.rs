//! Matching compiled templates against an utterance, under a step budget.

use super::grammar::{GrammarError, Item, compile};
use super::intent::Intent;
use super::types::{Match, SlotValue};
use super::values::{candidates, normalize};
use crate::lang::Language;

/// Search steps allowed per utterance before giving up (and falling through to the LLM).
const STEP_BUDGET: usize = 20_000;

fn match_items(
    lang: &Language,
    items: &[Item],
    toks: &[String],
    pos: usize,
    caps: &mut Vec<(String, SlotValue)>,
    steps: &mut usize,
) -> bool {
    *steps += 1;
    if *steps > STEP_BUDGET {
        return false;
    }
    match items.split_first() {
        None => pos == toks.len(),
        Some((Item::Word(w), rest)) => {
            toks.get(pos).is_some_and(|t| t == w)
                && match_items(lang, rest, toks, pos + 1, caps, steps)
        }
        Some((Item::Slot(name, kind), rest)) => {
            let remaining = toks.get(pos..).unwrap_or(&[]);
            for (used, value) in candidates(lang, *kind, remaining) {
                caps.push((name.clone(), value));
                if match_items(lang, rest, toks, pos + used, caps, steps) {
                    return true;
                }
                caps.pop();
            }
            false
        }
    }
}

#[derive(Debug)]
pub(super) struct Compiled<I> {
    intent: I,
    patterns: Vec<Vec<Item>>,
}

/// A set of compiled intents, matched in a language's words.
#[derive(Debug)]
pub struct Matcher<I = Intent> {
    lang: &'static Language,
    pub(super) intents: Vec<Compiled<I>>,
}

impl<I: Copy> Matcher<I> {
    /// Compile intents given as `(intent, templates)`, to be matched with `lang`'s number, duration and
    /// clock words. Earlier intents win ties, so list the more specific ones first.
    ///
    /// # Errors
    /// [`GrammarError`] for a malformed template.
    pub fn new(lang: &'static Language, specs: &[(I, &[&str])]) -> Result<Self, GrammarError> {
        let mut intents = Vec::with_capacity(specs.len());
        for (intent, templates) in specs {
            let mut patterns = Vec::new();
            for t in *templates {
                patterns.extend(compile(t)?);
            }
            intents.push(Compiled {
                intent: *intent,
                patterns,
            });
        }
        Ok(Self { lang, intents })
    }

    /// Recognise a whole utterance, or `None` to fall through to the LLM.
    #[must_use]
    pub fn matches(&self, utterance: &str) -> Option<Match<I>> {
        let toks = normalize(self.lang, utterance);
        if toks.is_empty() {
            return None;
        }
        let mut steps = 0;
        for intent in &self.intents {
            for pattern in &intent.patterns {
                let mut caps = Vec::new();
                if match_items(self.lang, pattern, &toks, 0, &mut caps, &mut steps) {
                    return Some(Match {
                        intent: intent.intent,
                        slots: caps,
                    });
                }
                if steps > STEP_BUDGET {
                    return None;
                }
            }
        }
        None
    }
}

impl Matcher {
    /// The language's own grammar: stop, volume, music controls, timers, alarms, reminders, time and date.
    #[must_use]
    pub fn builtin(lang: &'static Language) -> Self {
        // The built-in templates are tested to compile; an error here is a programming mistake.
        Self::new(lang, lang.grammar).unwrap_or(Self {
            lang,
            intents: Vec::new(),
        })
    }
}
