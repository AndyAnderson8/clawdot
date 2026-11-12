//! Local fast-path intent matcher (REQ-AST-010, REQ-NFR-111).
//!
//! Simple commands ("set a timer for ten minutes", "volume up", "stop") are recognised here, without an
//! LLM round trip, so they are instant, free, and work offline. Anything not recognised falls through
//! to the LLM. The matcher is authoritative for its own grammar and bounded in time: matching is a
//! backtracking search with a hard step budget, so a pathological utterance can never stall the device.
//!
//! Templates use a small hassil-style syntax:
//!
//! * `word`          a literal word
//! * `(a|b c)`       alternatives
//! * `[a b]`         optional
//! * `{name}`        a slot; `{name:kind}` picks the kind (`text`, `number`, `duration`, `time`),
//!   and a bare name that is itself a kind uses that kind
//! * `word[s]`       a word with an optional glued suffix, meaning "word" or "words"

use std::fmt;

/// Search steps allowed per utterance before giving up (and falling through to the LLM).
const STEP_BUDGET: usize = 20_000;
/// Linear patterns one template may expand to.
const MAX_EXPANSIONS: usize = 512;
/// Groups inside groups a template may have. The parser and the expander recurse once per level.
const MAX_NESTING: usize = 8;

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

/// A recognised command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Intent name, for example `timer.set`.
    pub intent: String,
    /// Captured slots in template order.
    pub slots: Vec<(String, SlotValue)>,
}

impl Match {
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

/// A template could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarError(pub String);

impl fmt::Display for GrammarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad template: {}", self.0)
    }
}

impl std::error::Error for GrammarError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Duration,
    Time,
}

#[derive(Debug, Clone)]
enum Node {
    Word(String),
    Slot(String, Kind),
    Alt(Vec<Vec<Node>>),
    Opt(Vec<Vec<Node>>),
}

#[derive(Debug, Clone)]
enum Item {
    Word(String),
    Slot(String, Kind),
}

/// Append a node to the alternative being built (there is always at least one).
fn push_node(alts: &mut [Vec<Node>], node: Node) {
    if let Some(current) = alts.last_mut() {
        current.push(node);
    }
}

fn parse_alternatives(
    depth: usize,
    it: &mut std::iter::Peekable<std::str::Chars<'_>>,
    close: Option<char>,
) -> Result<Vec<Vec<Node>>, GrammarError> {
    if depth > MAX_NESTING {
        return Err(GrammarError("groups are nested too deeply".into()));
    }
    let mut alts: Vec<Vec<Node>> = vec![Vec::new()];
    loop {
        let Some(&c) = it.peek() else {
            return if close.is_some() {
                Err(GrammarError("unclosed group".into()))
            } else {
                Ok(alts)
            };
        };
        match c {
            c if c.is_whitespace() => {
                it.next();
            }
            ')' | ']' => {
                if Some(c) == close {
                    it.next();
                    return Ok(alts);
                }
                return Err(GrammarError(format!("unexpected `{c}`")));
            }
            '|' => {
                it.next();
                alts.push(Vec::new());
            }
            '(' | '[' => {
                it.next();
                let (end, optional) = if c == '(' { (')', false) } else { (']', true) };
                let inner = parse_alternatives(depth + 1, it, Some(end))?;
                let node = if optional {
                    Node::Opt(inner)
                } else {
                    Node::Alt(inner)
                };
                push_node(&mut alts, node);
            }
            '{' => {
                it.next();
                let mut body = String::new();
                loop {
                    match it.next() {
                        Some('}') => break,
                        Some(ch) => body.push(ch),
                        None => return Err(GrammarError("unclosed slot".into())),
                    }
                }
                let (name, kind) = match body.split_once(':') {
                    Some((n, k)) => (n.trim(), k.trim()),
                    None => (body.trim(), body.trim()),
                };
                let kind = match kind {
                    "number" => Kind::Number,
                    "duration" => Kind::Duration,
                    "time" => Kind::Time,
                    _ => Kind::Text,
                };
                if name.is_empty() {
                    return Err(GrammarError("slot has no name".into()));
                }
                push_node(&mut alts, Node::Slot(name.to_owned(), kind));
            }
            _ => {
                let mut word = String::new();
                while let Some(&ch) = it.peek() {
                    if ch.is_whitespace() || "()[]{}|".contains(ch) {
                        break;
                    }
                    word.push(ch);
                    it.next();
                }
                let word = word.to_lowercase();
                if it.peek() == Some(&'[') {
                    // `word[s]` is "word" or "words": an optional suffix glued to the word.
                    it.next();
                    let inner = parse_alternatives(depth + 1, it, Some(']'))?;
                    let suffix = match inner.as_slice() {
                        [alt] => match alt.as_slice() {
                            [Node::Word(s)] => Some(s.clone()),
                            _ => None,
                        },
                        _ => None,
                    };
                    let Some(suffix) = suffix else {
                        return Err(GrammarError(format!(
                            "`{word}[...]` must be a plain suffix"
                        )));
                    };
                    let longer = format!("{word}{suffix}");
                    let either = Node::Alt(vec![vec![Node::Word(word)], vec![Node::Word(longer)]]);
                    push_node(&mut alts, either);
                } else {
                    push_node(&mut alts, Node::Word(word));
                }
            }
        }
    }
}

fn cross(left: Vec<Vec<Item>>, right: &[Vec<Item>]) -> Result<Vec<Vec<Item>>, GrammarError> {
    if left.len().saturating_mul(right.len().max(1)) > MAX_EXPANSIONS {
        return Err(GrammarError("template expands to too many patterns".into()));
    }
    let mut out = Vec::with_capacity(left.len() * right.len().max(1));
    for l in &left {
        if right.is_empty() {
            out.push(l.clone());
        }
        for r in right {
            let mut joined = l.clone();
            joined.extend(r.iter().cloned());
            out.push(joined);
        }
    }
    Ok(out)
}

fn expand_alts(alts: &[Vec<Node>]) -> Result<Vec<Vec<Item>>, GrammarError> {
    let mut out = Vec::new();
    for alt in alts {
        out.extend(expand_seq(alt)?);
        if out.len() > MAX_EXPANSIONS {
            return Err(GrammarError("template expands to too many patterns".into()));
        }
    }
    Ok(out)
}

fn expand_seq(nodes: &[Node]) -> Result<Vec<Vec<Item>>, GrammarError> {
    let mut acc: Vec<Vec<Item>> = vec![Vec::new()];
    for node in nodes {
        acc = match node {
            Node::Word(w) => cross(acc, &[vec![Item::Word(w.clone())]])?,
            Node::Slot(n, k) => cross(acc, &[vec![Item::Slot(n.clone(), *k)]])?,
            Node::Alt(alts) => cross(acc, &expand_alts(alts)?)?,
            Node::Opt(alts) => {
                let mut subs = expand_alts(alts)?;
                subs.push(Vec::new());
                cross(acc, &subs)?
            }
        };
    }
    Ok(acc)
}

fn compile(template: &str) -> Result<Vec<Vec<Item>>, GrammarError> {
    let mut it = template.chars().peekable();
    let alts = parse_alternatives(0, &mut it, None)?;
    let patterns = expand_alts(&alts)?;
    if patterns.iter().any(Vec::is_empty) {
        return Err(GrammarError(format!(
            "template `{template}` can match nothing"
        )));
    }
    Ok(patterns)
}

// ---------------------------------------------------------------------------------------------
// Normalisation and value parsing
// ---------------------------------------------------------------------------------------------

const LEADING_FILLERS: &[&[&str]] = &[
    &["please"],
    &["hey"],
    &["ok"],
    &["okay"],
    &["can", "you"],
    &["could", "you"],
    &["would", "you"],
    &["will", "you"],
    &["go", "ahead", "and"],
    &["i", "want", "you", "to"],
    &["i'd", "like", "you", "to"],
    &["i", "would", "like", "you", "to"],
];
const TRAILING_FILLERS: &[&[&str]] = &[&["please"], &["thanks"], &["thank", "you"]];

/// Lower-case, strip punctuation, and drop polite filler words.
#[must_use]
pub fn normalize(utterance: &str) -> Vec<String> {
    let mut s = String::with_capacity(utterance.len());
    for c in utterance.to_lowercase().chars() {
        match c {
            '\u{2019}' => s.push('\''),
            '.' => {}
            c if c.is_alphanumeric() || c == '\'' || c == ':' => s.push(c),
            _ => s.push(' '),
        }
    }
    let mut toks: Vec<String> = s.split_whitespace().map(str::to_owned).collect();
    loop {
        let before = toks.len();
        for f in LEADING_FILLERS {
            if toks.len() > f.len() && toks.iter().zip(f.iter()).all(|(a, b)| a == b) {
                toks.drain(..f.len());
            }
        }
        for f in TRAILING_FILLERS {
            if toks.len() > f.len()
                && toks[toks.len() - f.len()..]
                    .iter()
                    .zip(f.iter())
                    .all(|(a, b)| a == b)
            {
                let new_len = toks.len() - f.len();
                toks.truncate(new_len);
            }
        }
        if toks.len() == before {
            return toks;
        }
    }
}

fn small_number(tok: &str) -> Option<u32> {
    Some(match tok {
        "zero" | "oh" => 0,
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "thirteen" => 13,
        "fourteen" => 14,
        "fifteen" => 15,
        "sixteen" => 16,
        "seventeen" => 17,
        "eighteen" => 18,
        "nineteen" => 19,
        _ => return None,
    })
}

fn tens(tok: &str) -> Option<u32> {
    Some(match tok {
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        _ => return None,
    })
}

/// A number at the start of `toks`: digits, or words up to "one hundred". Returns `(value, tokens used)`.
fn parse_number(toks: &[String]) -> Option<(u32, usize)> {
    let first = toks.first()?;
    if first.len() <= 4 && first.chars().all(|c| c.is_ascii_digit()) {
        return first.parse().ok().map(|n| (n, 1));
    }
    if first == "a" && toks.get(1).is_some_and(|t| t == "hundred") {
        return Some((100, 2));
    }
    if let Some(t) = tens(first) {
        if let Some(o) = toks
            .get(1)
            .and_then(|t| small_number(t))
            .filter(|o| (1..=9).contains(o))
        {
            return Some((t + o, 2));
        }
        return Some((t, 1));
    }
    let base = small_number(first)?;
    if base < 10 && toks.get(1).is_some_and(|t| t == "hundred") {
        return Some((base * 100, 2));
    }
    Some((base, 1))
}

fn unit_seconds(tok: &str) -> Option<i64> {
    Some(match tok {
        "second" | "seconds" | "sec" | "secs" => 1,
        "minute" | "minutes" | "min" | "mins" => 60,
        "hour" | "hours" | "hr" | "hrs" => 3600,
        "day" | "days" => 86_400,
        _ => return None,
    })
}

/// One "quantity unit" chunk, for example "ten minutes" or "an hour".
fn duration_chunk(toks: &[String]) -> Option<(usize, i64, i64)> {
    // (tokens used, seconds, unit seconds)
    if toks.first().is_some_and(|t| t == "half")
        && toks.get(1).is_some_and(|t| t == "an")
        && toks.get(2).is_some_and(|t| t == "hour")
    {
        return Some((3, 1800, 3600));
    }
    let (qty, used) = match toks.first().map(String::as_str) {
        Some("a" | "an") => (1_i64, 1),
        _ => {
            let (n, used) = parse_number(toks)?;
            (i64::from(n), used)
        }
    };
    let unit = unit_seconds(toks.get(used)?)?;
    Some((used + 1, qty * unit, unit))
}

fn duration_candidates(toks: &[String]) -> Vec<(usize, i64)> {
    let mut results = Vec::new();
    let Some((mut used, mut total, mut last_unit)) = duration_chunk(toks) else {
        return results;
    };
    results.push((used, total));
    loop {
        // "... and a half" after hours or minutes
        if toks.get(used).is_some_and(|t| t == "and")
            && toks.get(used + 1).is_some_and(|t| t == "a")
            && toks.get(used + 2).is_some_and(|t| t == "half")
            && last_unit >= 60
        {
            used += 3;
            total += last_unit / 2;
            results.push((used, total));
            break;
        }
        let skip = usize::from(toks.get(used).is_some_and(|t| t == "and"));
        match duration_chunk(toks.get(used + skip..).unwrap_or(&[])) {
            Some((n, secs, unit)) if unit < last_unit => {
                used += skip + n;
                total += secs;
                last_unit = unit;
                results.push((used, total));
            }
            _ => break,
        }
    }
    results.reverse();
    results
}

fn parse_clock_token(tok: &str) -> Option<(u8, u8, Option<Meridiem>)> {
    let (body, meridiem) = if let Some(b) = tok.strip_suffix("am") {
        (b, Some(Meridiem::Am))
    } else if let Some(b) = tok.strip_suffix("pm") {
        (b, Some(Meridiem::Pm))
    } else {
        (tok, None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h.parse::<u8>().ok()?, m.parse::<u8>().ok()?),
        Some(_) => return None,
        None if meridiem.is_some() => (body.parse::<u8>().ok()?, 0),
        None => return None,
    };
    valid_clock(h, m, meridiem).then_some((h, m, meridiem))
}

fn valid_clock(h: u8, m: u8, meridiem: Option<Meridiem>) -> bool {
    m < 60 && h <= 24 && (meridiem.is_none() || (1..=12).contains(&h))
}

fn meridiem_at(toks: &[String], i: usize) -> Option<Meridiem> {
    match toks.get(i).map(String::as_str) {
        Some("am") => Some(Meridiem::Am),
        Some("pm") => Some(Meridiem::Pm),
        _ => None,
    }
}

fn time_candidates(toks: &[String]) -> Vec<(usize, Time)> {
    let mut out: Vec<(usize, Time)> = Vec::new();
    let Some(first) = toks.first() else {
        return out;
    };
    let mut push = |used: usize, hour: u8, minute: u8, meridiem: Option<Meridiem>| {
        if valid_clock(hour, minute, meridiem) {
            out.push((
                used,
                Time {
                    hour,
                    minute,
                    meridiem,
                },
            ));
        }
    };
    match first.as_str() {
        "noon" => push(1, 12, 0, Some(Meridiem::Pm)),
        "midnight" => push(1, 12, 0, Some(Meridiem::Am)),
        _ => {}
    }
    if let Some((h, m, mer)) = parse_clock_token(first) {
        if mer.is_some() {
            push(1, h, m, mer);
        } else {
            if let Some(mer) = meridiem_at(toks, 1) {
                push(2, h, m, Some(mer));
            }
            push(1, h, m, None);
        }
    } else if let Some((h, hused)) = parse_number(toks).filter(|(h, _)| *h <= 24) {
        let h = h as u8;
        // "seven thirty [pm]" / "7 30 [pm]" / "seven oh five"
        let minute = if toks.get(hused).is_some_and(|t| t == "oh" || t == "o") {
            parse_number(&toks[hused + 1..])
                .filter(|(m, n)| *n == 1 && (1..=9).contains(m))
                .map(|(m, n)| (m, n + 1))
        } else {
            parse_number(toks.get(hused..).unwrap_or(&[])).filter(|(m, _)| (10..=59).contains(m))
        };
        if let Some((m, mused)) = minute {
            let used = hused + mused;
            if let Some(mer) = meridiem_at(toks, used) {
                push(used + 1, h, m as u8, Some(mer));
            }
            push(used, h, m as u8, None);
        }
        if matches!(
            toks.get(hused).map(String::as_str),
            Some("o'clock" | "oclock")
        ) {
            if let Some(mer) = meridiem_at(toks, hused + 1) {
                push(hused + 2, h, 0, Some(mer));
            }
            push(hused + 1, h, 0, None);
        }
        if let Some(mer) = meridiem_at(toks, hused) {
            push(hused + 1, h, 0, Some(mer));
        }
        // a bare hour only counts for plain digits or words 1..=12 followed by something the template wants
        if h >= 1 {
            push(hused, h, 0, None);
        }
    }
    out.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
    out.dedup_by_key(|(used, t)| (*used, t.hour, t.minute, t.meridiem));
    out
}

fn candidates(kind: Kind, toks: &[String]) -> Vec<(usize, SlotValue)> {
    match kind {
        Kind::Number => parse_number(toks)
            .map(|(n, used)| vec![(used, SlotValue::Number(i64::from(n)))])
            .unwrap_or_default(),
        Kind::Duration => duration_candidates(toks)
            .into_iter()
            .map(|(used, secs)| (used, SlotValue::Duration(secs)))
            .collect(),
        Kind::Time => time_candidates(toks)
            .into_iter()
            .map(|(used, t)| (used, SlotValue::Time(t)))
            .collect(),
        // A lone article or pronoun is never a useful label ("set a timer" must not name it "a").
        Kind::Text => (1..=toks.len())
            .filter(|&n| {
                !(n == 1
                    && matches!(
                        toks[0].as_str(),
                        "a" | "an" | "the" | "my" | "all" | "of" | "it" | "that" | "this"
                    ))
            })
            .map(|n| (n, SlotValue::Text(toks[..n].join(" "))))
            .collect(),
    }
}

// ---------------------------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------------------------

fn match_items(
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
            toks.get(pos).is_some_and(|t| t == w) && match_items(rest, toks, pos + 1, caps, steps)
        }
        Some((Item::Slot(name, kind), rest)) => {
            let remaining = toks.get(pos..).unwrap_or(&[]);
            for (used, value) in candidates(*kind, remaining) {
                caps.push((name.clone(), value));
                if match_items(rest, toks, pos + used, caps, steps) {
                    return true;
                }
                caps.pop();
            }
            false
        }
    }
}

#[derive(Debug)]
struct Compiled {
    intent: String,
    patterns: Vec<Vec<Item>>,
}

/// A set of compiled intents.
#[derive(Debug)]
pub struct Matcher {
    intents: Vec<Compiled>,
}

impl Matcher {
    /// Compile intents given as `(name, templates)`. Earlier intents win ties, so list the more specific
    /// ones first.
    ///
    /// # Errors
    /// [`GrammarError`] for a malformed template.
    pub fn new(specs: &[(&str, &[&str])]) -> Result<Self, GrammarError> {
        let mut intents = Vec::with_capacity(specs.len());
        for (name, templates) in specs {
            let mut patterns = Vec::new();
            for t in *templates {
                patterns.extend(compile(t)?);
            }
            intents.push(Compiled {
                intent: (*name).to_owned(),
                patterns,
            });
        }
        Ok(Self { intents })
    }

    /// The built-in grammar: stop, volume, music controls, timers, alarms, reminders, time and date.
    #[must_use]
    pub fn builtin() -> Self {
        // The built-in templates are tested to compile; an error here is a programming mistake.
        Self::new(BUILTIN).unwrap_or(Self {
            intents: Vec::new(),
        })
    }

    /// Recognise a whole utterance, or `None` to fall through to the LLM.
    #[must_use]
    pub fn matches(&self, utterance: &str) -> Option<Match> {
        let toks = normalize(utterance);
        if toks.is_empty() {
            return None;
        }
        let mut steps = 0;
        for intent in &self.intents {
            for pattern in &intent.patterns {
                let mut caps = Vec::new();
                if match_items(pattern, &toks, 0, &mut caps, &mut steps) {
                    return Some(Match {
                        intent: intent.intent.clone(),
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

/// Built-in intents. Names are the contract with the tool layer.
pub const BUILTIN: &[(&str, &[&str])] = &[
    (
        "stop",
        &[
            "stop",
            "cancel",
            "never mind",
            "nevermind",
            "be quiet",
            "quiet",
            "enough",
            "that's enough",
            "shut up",
            "stop (it|that|talking|speaking|ringing)",
            "silence [it|that|the alarm]",
        ],
    ),
    (
        "inbox.status",
        &[
            "what are my agents (up to|doing)",
            "(any|are there any) (updates|news|messages|notifications)",
            "what's new",
            "anything new",
            "what did i miss",
            "check my (agents|inbox)",
        ],
    ),
    (
        "inbox.details",
        &[
            "(give me|tell me|read me) [the|more] (details|info|information)",
            "more (details|info|information)",
            "tell me more",
            "what did (it|they|claude) (say|do)",
            "read (it|that|the whole thing)",
            "go on",
        ],
    ),
    (
        "inbox.snooze",
        &[
            "snooze [it|that|this] [for {duration}]",
            "remind me (later|again) [in {duration}]",
            "ask me (later|again) [in {duration}]",
            "not now",
            "later",
        ],
    ),
    (
        "inbox.ack",
        &[
            "(thanks|thank you) [i'll|i will] (handle|take care of|deal with|take) (it|that|this) [from here]",
            "i'll (handle|take care of|deal with|take) (it|that|this) [from here]",
            "i (got|have) (it|this|that)",
            "i've got (it|this|that)",
            "(thanks|thank you) [i got it]",
            "got it [thanks]",
            "that's all [thanks]",
        ],
    ),
    (
        "inbox.dismiss",
        &[
            "dismiss [it|that|this|the alarm|the notification|the message]",
            "(clear|ignore|forget) (it|that|this)",
        ],
    ),
    (
        "inbox.instruct",
        &[
            "tell (it|claude|them|the agent) to {text}",
            "tell (it|claude|them|the agent) {text}",
            "(keep going|carry on|continue) (and|with|but) {text}",
            "(keep going|carry on|continue)",
            "reply {text}",
        ],
    ),
    (
        "media.pause",
        &[
            "pause [the] [music|song|playback|radio]",
            "stop the (music|song|playback|radio)",
        ],
    ),
    (
        "media.resume",
        &["(resume|continue|unpause|play) [the] [music|song|playback|radio]"],
    ),
    (
        "media.next",
        &[
            "(next|skip) [the] [song|track|station]",
            "skip this [song|track]",
        ],
    ),
    (
        "media.previous",
        &[
            "(previous|go back) [song|track]",
            "play the previous (song|track)",
        ],
    ),
    (
        "media.play",
        &[
            "play [some] {name:text} (radio|station|music)",
            "play [some|the] {name:text}",
            "put on [some|the] {name:text}",
            "(listen to|i want to hear) [some|the] {name:text}",
        ],
    ),
    (
        "volume.set",
        &[
            "(set|turn|put) [the] volume (to|at) {level:number} [percent]",
            "volume {level:number} [percent]",
            "(set|turn) it to {level:number} [percent]",
        ],
    ),
    (
        "volume.up",
        &[
            "volume up",
            "louder",
            "turn it up",
            "speak up",
            "(turn|make|bring) [the] volume up",
            "(increase|raise) [the] volume",
            "(turn|make) it louder",
        ],
    ),
    (
        "volume.down",
        &[
            "volume down",
            "quieter",
            "softer",
            "turn it down",
            "(turn|make|bring) [the] volume down",
            "(decrease|lower|reduce) [the] volume",
            "(turn|make) it (quieter|softer)",
        ],
    ),
    (
        "timer.cancel",
        &[
            "(cancel|stop|delete|clear|remove) [the|my|all] [{label:text}] timer[s]",
            "(cancel|stop|delete|clear|remove) [all] [of] [my] timers",
        ],
    ),
    (
        "timer.query",
        &[
            "how much time (is|do i have) (left|remaining) [on [the] [{label:text}] timer]",
            "how long (is|until) (left|remaining|the timer) [on [the] [{label:text}] timer]",
            "(check|show) [the] [{label:text}] timer",
            "(list|show|read|tell me|give me) [me] [all] [of] [the|my] [running|active] timers",
            "what [are] [the|my] [running|active] timers [(do i have|are there|are running|are set|are active)]",
            "what timers (do i have|are (there|running|set|active)) [running|set|right now]",
            "which timers (do i have|are (there|running|set|active))",
            "(do i have|are there) any [running|active] timers [running|set|right now]",
            "how many timers (do i have|are (there|running|set))",
        ],
    ),
    (
        "timer.set",
        &[
            "(set|start|create|make) [a|an|the] timer for {duration} (called|named) {label:text}",
            "(set|start|create|make) [a|an|the] {label:text} timer for {duration}",
            "(set|start|create|make) [a|an|the] timer for {duration}",
            "(set|start|create|make) [a|an|the] {duration} [{label:text}] timer",
            "(set|start|create|make) [a|an|the] timer {duration}",
            "timer for {duration}",
            "{duration} timer",
            "time me for {duration}",
        ],
    ),
    (
        "stopwatch.start",
        &[
            "(start|begin|run|use) [a|the|my] [new] (stopwatch|stop watch)",
            "(set up|set) [a|the|my] [new] (stopwatch|stop watch)",
            "(stopwatch|stop watch) start",
            "(start|begin) timing [me|it|this]",
            "time (me|this|it)",
        ],
    ),
    (
        "stopwatch.lap",
        &[
            "lap",
            "split",
            "lap time",
            "(next|new|another) (lap|split)",
            "(mark|record|take|log|do|add|start) [a|an|another|the|next|new] (lap|split)",
            "(lap|split) [the|my] (stopwatch|stop watch)",
            "(stopwatch|stop watch) (lap|split)",
        ],
    ),
    (
        "stopwatch.stop",
        &[
            "(stop|end|finish) [the|my] (stopwatch|stop watch)",
            "(stopwatch|stop watch) stop",
            "stop timing [me|it|this]",
        ],
    ),
    (
        "stopwatch.read",
        &[
            "how long (has|is) [the|my] (stopwatch|stop watch) (been running|running|at|on)",
            "how long (on|is on) [the|my] (stopwatch|stop watch)",
            "how much time (is on|has passed on|has elapsed on) [the|my] (stopwatch|stop watch)",
            "(what's|what is) [on|at] [the|my] (stopwatch|stop watch) [at|reading|time|say|says|show|shows]",
            "(check|read|show|tell me|give me) [me] [the|my] (stopwatch|stop watch) [time|reading]",
            "(stopwatch|stop watch) (time|reading|check)",
        ],
    ),
    (
        "stopwatch.reset",
        &["(reset|clear|cancel|delete|remove|zero) [the|my] (stopwatch|stop watch)"],
    ),
    (
        "alarm.cancel",
        &["(cancel|stop|delete|clear|remove|turn off) [the|my|all] [of] [my] alarm[s]"],
    ),
    (
        "alarm.set_weekdays",
        &[
            "(set|create|make) [a|an] [alarm] (every weekday|on weekdays|for weekdays) (at|for) {time}",
            "(set|create|make) [a|an] weekday alarm (at|for) {time}",
        ],
    ),
    (
        "alarm.set_weekend",
        &["(set|create|make) [a|an] weekend alarm (at|for) {time}"],
    ),
    (
        "alarm.set_daily",
        &[
            "(set|create|make) [a|an] (daily|every day|repeating) alarm (at|for) {time}",
            "(set|create|make) [a|an] alarm (at|for) {time} every day",
        ],
    ),
    (
        "alarm.set",
        &[
            "(set|create|make) [a|an] alarm (at|for) {time} (called|named) {label:text}",
            "(set|create|make) [a|an] alarm (at|for) {time}",
            "(wake me|wake me up) (at|up at) {time}",
            "alarm (at|for) {time}",
        ],
    ),
    (
        "reminder.set_in",
        &[
            "remind me (in|after) {duration} to {text}",
            "remind me to {text} in {duration}",
            "remind me to {text} after {duration}",
        ],
    ),
    (
        "reminder.set_at",
        &[
            "remind me (at|by) {time} to {text}",
            "remind me to {text} (at|by) {time}",
        ],
    ),
    (
        "time.query",
        &[
            "what time is it [now]",
            "what's the time [now]",
            "what is the time [now]",
            "tell me the time",
            "what time",
        ],
    ),
    (
        "date.query",
        &[
            "what's the date [today]",
            "what is the date [today]",
            "what day is it [today]",
            "what's today's date",
            "what's today",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn m(utterance: &str) -> Option<Match> {
        Matcher::builtin().matches(utterance)
    }

    fn intent(utterance: &str) -> Option<String> {
        m(utterance).map(|x| x.intent)
    }

    #[test]
    fn the_built_in_grammar_compiles() {
        assert!(!Matcher::builtin().intents.is_empty());
        assert!(Matcher::new(BUILTIN).is_ok());
    }

    #[test]
    fn normalisation_strips_punctuation_and_politeness() {
        assert_eq!(normalize("Hey, could you stop, please?"), vec!["stop"]);
        assert_eq!(
            normalize("Set an alarm for 7 a.m."),
            vec!["set", "an", "alarm", "for", "7", "am"]
        );
        assert_eq!(
            normalize("ten-minute timer"),
            vec!["ten", "minute", "timer"]
        );
    }

    #[test]
    fn numbers_parse_from_digits_and_words() {
        let n = |s: &str| parse_number(&normalize(s)).map(|(v, _)| v);
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
            assert_eq!(got.intent, "timer.set", "{u}");
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
        assert_eq!(intent("cancel the timer").as_deref(), Some("timer.cancel"));
        assert_eq!(intent("cancel all timers").as_deref(), Some("timer.cancel"));
        assert_eq!(
            m("cancel the pasta timer").unwrap().text("label"),
            Some("pasta")
        );
        assert_eq!(
            intent("how much time is left").as_deref(),
            Some("timer.query")
        );
        assert_eq!(
            intent("how much time is left on the pasta timer").as_deref(),
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
            assert_eq!(intent(u).as_deref(), Some("timer.query"), "{u}");
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
                assert_eq!(intent(u).as_deref(), Some(intent_name), "{u}");
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
            assert_eq!(intent(u).as_deref(), want, "{u}");
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
                got.as_deref().is_none_or(|i| !i.starts_with("stopwatch.")),
                "{u} -> {got:?}"
            );
        }
    }

    #[test]
    fn alarms() {
        let a = m("set an alarm for 7 am").unwrap();
        assert_eq!(a.intent, "alarm.set");
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
            intent("set a weekday alarm for 6:30 am").as_deref(),
            Some("alarm.set_weekdays")
        );
        assert_eq!(
            intent("set a daily alarm for seven").as_deref(),
            Some("alarm.set_daily")
        );
        assert_eq!(
            intent("set a weekend alarm for 9 am").as_deref(),
            Some("alarm.set_weekend")
        );
        assert_eq!(intent("cancel my alarm").as_deref(), Some("alarm.cancel"));
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
        assert_eq!(a.intent, "reminder.set_in");
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
        assert_eq!(c.intent, "reminder.set_at");
        assert_eq!(c.text("text"), Some("pick up the kids"));
    }

    #[test]
    fn stop_volume_and_media_controls() {
        assert_eq!(intent("stop").as_deref(), Some("stop"));
        assert_eq!(intent("Never mind.").as_deref(), Some("stop"));
        assert_eq!(intent("volume up").as_deref(), Some("volume.up"));
        assert_eq!(intent("turn it down").as_deref(), Some("volume.down"));
        assert_eq!(
            m("set the volume to 40 percent").unwrap().number("level"),
            Some(40)
        );
        assert_eq!(m("volume sixty").unwrap().number("level"), Some(60));
        assert_eq!(intent("pause the music").as_deref(), Some("media.pause"));
        assert_eq!(intent("stop the music").as_deref(), Some("media.pause"));
        assert_eq!(intent("resume").as_deref(), Some("media.resume"));
        assert_eq!(intent("skip this song").as_deref(), Some("media.next"));
        assert_eq!(intent("play some jazz").as_deref(), Some("media.play"));
        assert_eq!(
            intent("put on the bbc world service").as_deref(),
            Some("media.play")
        );
        // "play the music" is a resume, not a station called "music".
        assert_eq!(intent("play the music").as_deref(), Some("media.resume"));
    }

    #[test]
    fn inbox_commands_are_recognised() {
        assert_eq!(
            intent("what are my agents up to").as_deref(),
            Some("inbox.status")
        );
        assert_eq!(intent("any updates").as_deref(), Some("inbox.status"));
        assert_eq!(
            intent("give me the details").as_deref(),
            Some("inbox.details")
        );
        assert_eq!(intent("tell me more").as_deref(), Some("inbox.details"));
        assert_eq!(
            intent("thanks I'll handle it").as_deref(),
            Some("inbox.ack")
        );
        assert_eq!(intent("I've got it").as_deref(), Some("inbox.ack"));
        assert_eq!(intent("dismiss it").as_deref(), Some("inbox.dismiss"));
        assert_eq!(intent("dismiss").as_deref(), Some("inbox.dismiss"));
        let s = m("snooze for twenty minutes").unwrap();
        assert_eq!(
            (s.intent.as_str(), s.duration("duration")),
            ("inbox.snooze", Some(1200))
        );
        assert_eq!(intent("not now").as_deref(), Some("inbox.snooze"));
        let i = m("tell it to also add unit tests").unwrap();
        assert_eq!(
            (i.intent.as_str(), i.text("text")),
            ("inbox.instruct", Some("also add unit tests"))
        );
        let j = m("keep going and fix the failing test").unwrap();
        assert_eq!(j.text("text"), Some("fix the failing test"));
        assert_eq!(intent("keep going").as_deref(), Some("inbox.instruct"));
    }

    #[test]
    fn time_and_date_are_answered_locally() {
        assert_eq!(intent("what time is it").as_deref(), Some("time.query"));
        assert_eq!(
            intent("What's the date today?").as_deref(),
            Some("date.query")
        );
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
            assert!(Matcher::new(&[("x", &[bad])]).is_err(), "{bad}");
        }
    }

    #[test]
    fn deeply_nested_templates_are_refused_instead_of_overflowing_the_stack() {
        // Templates are compiled in today, but packs will bring them from outside (REQ-EXT-002).
        let nested = format!("{}a{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(Matcher::new(&[("x", &[nested.as_str()])]).is_err());
        let optional = format!("{}a{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(Matcher::new(&[("x", &[optional.as_str()])]).is_err());
        // Ordinary nesting still works.
        assert!(Matcher::new(&[("x", &["(a (b [c]) | d) e"])]).is_ok());
    }

    #[test]
    fn explosive_templates_are_refused() {
        let t = "[a] [b] [c] [d] [e] [f] [g] [h] [i] [j] x";
        assert!(Matcher::new(&[("x", &[t])]).is_err());
    }

    #[test]
    fn pathological_input_cannot_stall_the_matcher() {
        let matcher = Matcher::new(&[(
            "greedy",
            &["{a:text} {b:text} {c:text} {d:text} {e:text} {f:text} end"],
        )])
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
        let matcher = Matcher::builtin();
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
}
