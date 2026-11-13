//! Spoken-value parsing: normalisation, numbers, durations and clock times.

use super::grammar::Kind;
use super::types::{Meridiem, SlotValue, Time};
use crate::lang::Language;

/// Whether `toks` starts with exactly these words.
fn starts_with(toks: &[String], words: &[&str]) -> bool {
    toks.len() >= words.len() && toks.iter().zip(words).all(|(t, w)| t == w)
}

/// Lower-case, strip punctuation, and drop polite filler words.
#[must_use]
pub fn normalize(lang: &Language, utterance: &str) -> Vec<String> {
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
        for f in lang.leading_fillers {
            if toks.len() > f.len() && starts_with(&toks, f) {
                toks.drain(..f.len());
            }
        }
        for f in lang.trailing_fillers {
            if toks.len() > f.len() && starts_with(&toks[toks.len() - f.len()..], f) {
                let new_len = toks.len() - f.len();
                toks.truncate(new_len);
            }
        }
        if toks.len() == before {
            return toks;
        }
    }
}

fn word_value(table: &[(&str, u32)], tok: &str) -> Option<u32> {
    table.iter().find(|(w, _)| *w == tok).map(|(_, v)| *v)
}

/// A number at the start of `toks`: digits, or words up to "one hundred". Returns `(value, tokens used)`.
pub(super) fn parse_number(lang: &Language, toks: &[String]) -> Option<(u32, usize)> {
    let n = &lang.numbers;
    let first = toks.first()?;
    if first.len() <= 4 && first.chars().all(|c| c.is_ascii_digit()) {
        return first.parse().ok().map(|n| (n, 1));
    }
    if first == n.article_a && toks.get(1).is_some_and(|t| t == n.hundred) {
        return Some((100, 2));
    }
    if let Some(t) = word_value(n.tens, first) {
        if let Some(o) = toks
            .get(1)
            .and_then(|t| word_value(n.small, t))
            .filter(|o| (1..=9).contains(o))
        {
            return Some((t + o, 2));
        }
        return Some((t, 1));
    }
    let base = word_value(n.small, first)?;
    if base < 10 && toks.get(1).is_some_and(|t| t == n.hundred) {
        return Some((base * 100, 2));
    }
    Some((base, 1))
}

fn unit_seconds(lang: &Language, tok: &str) -> Option<i64> {
    lang.durations
        .units
        .iter()
        .find(|(words, _)| words.contains(&tok))
        .map(|(_, secs)| *secs)
}

/// One "quantity unit" chunk, for example "ten minutes" or "an hour".
fn duration_chunk(lang: &Language, toks: &[String]) -> Option<(usize, i64, i64)> {
    // (tokens used, seconds, unit seconds)
    let half = lang.durations.half_an_hour;
    if starts_with(toks, half) {
        return Some((half.len(), 1800, 3600));
    }
    let (qty, used) = match toks.first().map(String::as_str) {
        Some(a) if a == lang.numbers.article_a || a == lang.numbers.article_an => (1_i64, 1),
        _ => {
            let (n, used) = parse_number(lang, toks)?;
            (i64::from(n), used)
        }
    };
    let unit = unit_seconds(lang, toks.get(used)?)?;
    Some((used + 1, qty * unit, unit))
}

fn duration_candidates(lang: &Language, toks: &[String]) -> Vec<(usize, i64)> {
    let d = &lang.durations;
    let mut results = Vec::new();
    let Some((mut used, mut total, mut last_unit)) = duration_chunk(lang, toks) else {
        return results;
    };
    results.push((used, total));
    loop {
        // "... and a half" after hours or minutes
        if toks
            .get(used..)
            .is_some_and(|rest| starts_with(rest, d.and_a_half))
            && last_unit >= 60
        {
            used += d.and_a_half.len();
            total += last_unit / 2;
            results.push((used, total));
            break;
        }
        let skip = usize::from(toks.get(used).is_some_and(|t| t == d.and));
        match duration_chunk(lang, toks.get(used + skip..).unwrap_or(&[])) {
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

fn parse_clock_token(lang: &Language, tok: &str) -> Option<(u8, u8, Option<Meridiem>)> {
    let (body, meridiem) = if let Some(b) = tok.strip_suffix(lang.clock.am) {
        (b, Some(Meridiem::Am))
    } else if let Some(b) = tok.strip_suffix(lang.clock.pm) {
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

fn meridiem_at(lang: &Language, toks: &[String], i: usize) -> Option<Meridiem> {
    match toks.get(i).map(String::as_str) {
        Some(t) if t == lang.clock.am => Some(Meridiem::Am),
        Some(t) if t == lang.clock.pm => Some(Meridiem::Pm),
        _ => None,
    }
}

fn time_candidates(lang: &Language, toks: &[String]) -> Vec<(usize, Time)> {
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
    if first == lang.clock.noon {
        push(1, 12, 0, Some(Meridiem::Pm));
    } else if first == lang.clock.midnight {
        push(1, 12, 0, Some(Meridiem::Am));
    }
    if let Some((h, m, mer)) = parse_clock_token(lang, first) {
        if mer.is_some() {
            push(1, h, m, mer);
        } else {
            if let Some(mer) = meridiem_at(lang, toks, 1) {
                push(2, h, m, Some(mer));
            }
            push(1, h, m, None);
        }
    } else if let Some((h, hused)) = parse_number(lang, toks).filter(|(h, _)| *h <= 24) {
        let h = h as u8;
        // "seven thirty [pm]" / "7 30 [pm]" / "seven oh five"
        let minute = if toks
            .get(hused)
            .is_some_and(|t| lang.clock.oh.contains(&t.as_str()))
        {
            parse_number(lang, &toks[hused + 1..])
                .filter(|(m, n)| *n == 1 && (1..=9).contains(m))
                .map(|(m, n)| (m, n + 1))
        } else {
            parse_number(lang, toks.get(hused..).unwrap_or(&[]))
                .filter(|(m, _)| (10..=59).contains(m))
        };
        if let Some((m, mused)) = minute {
            let used = hused + mused;
            if let Some(mer) = meridiem_at(lang, toks, used) {
                push(used + 1, h, m as u8, Some(mer));
            }
            push(used, h, m as u8, None);
        }
        if toks
            .get(hused)
            .is_some_and(|t| lang.clock.oclock.contains(&t.as_str()))
        {
            if let Some(mer) = meridiem_at(lang, toks, hused + 1) {
                push(hused + 2, h, 0, Some(mer));
            }
            push(hused + 1, h, 0, None);
        }
        if let Some(mer) = meridiem_at(lang, toks, hused) {
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

pub(super) fn candidates(lang: &Language, kind: Kind, toks: &[String]) -> Vec<(usize, SlotValue)> {
    match kind {
        Kind::Number => parse_number(lang, toks)
            .map(|(n, used)| vec![(used, SlotValue::Number(i64::from(n)))])
            .unwrap_or_default(),
        Kind::Duration => duration_candidates(lang, toks)
            .into_iter()
            .map(|(used, secs)| (used, SlotValue::Duration(secs)))
            .collect(),
        Kind::Time => time_candidates(lang, toks)
            .into_iter()
            .map(|(used, t)| (used, SlotValue::Time(t)))
            .collect(),
        // A lone article or pronoun is never a useful label ("set a timer" must not name it "a").
        Kind::Text => (1..=toks.len())
            .filter(|&n| !(n == 1 && lang.numbers.not_a_label.contains(&toks[0].as_str())))
            .map(|n| (n, SlotValue::Text(toks[..n].join(" "))))
            .collect(),
    }
}
