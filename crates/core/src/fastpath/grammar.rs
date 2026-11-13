//! Template grammar: parsing, expansion and compiling.

use std::fmt;

/// Linear patterns one template may expand to.
const MAX_EXPANSIONS: usize = 512;
/// Groups inside groups a template may have. The parser and the expander recurse once per level.
const MAX_NESTING: usize = 8;

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
pub(super) enum Kind {
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
pub(super) enum Item {
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

pub(super) fn compile(template: &str) -> Result<Vec<Vec<Item>>, GrammarError> {
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
