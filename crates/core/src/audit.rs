//! The audit log of everything the assistant does (REQ-SEC-014), and the per-day action caps and dry-run
//! switch that sit beside it (REQ-SEC-015, REQ-SEC-016).
//!
//! One JSON object per line in `audit.jsonl` in the data directory (0600). The file is append-only and
//! size-capped: when it would pass the cap it is renamed to `audit.1.jsonl` (replacing the previous one) and a
//! new file starts, so the disk holds at most twice the cap. Start-up tolerates a crash: a half-written last
//! line or any damaged line is skipped, never an error.
//!
//! The log is written from the session loop with text that may come from outside (a tool result, a model's
//! arguments). So everything is made safe on the way in: secrets and tokens are replaced, each value is
//! clipped, control characters are removed, and nothing the user said or any audio is ever written.

use crate::sched::Ts;
use crate::session::Tier;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The records' format version.
pub const FORMAT: u32 = 1;
/// Default size of one log file; the disk holds at most twice this (current file plus the rotated one).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 256 * 1024;
/// A record's text values are clipped to this many characters.
pub const MAX_VALUE_CHARS: usize = 200;
/// The serialised arguments of one record are clipped to this many bytes.
const MAX_ARGS_BYTES: usize = 1_000;
/// Names of tools, triggers and keys are clipped to this many characters.
const MAX_NAME_CHARS: usize = 64;
/// How many records one page of the read endpoint holds, at most.
pub const MAX_PAGE: usize = 200;
/// Default page size.
pub const DEFAULT_PAGE: usize = 50;
/// A line longer than this is not read back (it cannot have been written by this code).
const MAX_LINE_BYTES: usize = 8 * 1024;

const REDACTED: &str = "[redacted]";

/// What the log says happened to a call. The strings are part of the file format.
pub mod decision {
    /// The call ran.
    pub const RAN: &str = "ran";
    /// The call was held until the user answers.
    pub const HELD: &str = "held";
    /// A turn the user did not start asked for an action; it was kept as a proposal.
    pub const PROPOSED: &str = "proposed";
    /// The user said yes and the held call ran.
    pub const APPROVED: &str = "approved";
    /// The user said no (or asked for something else) and the held call was dropped.
    pub const DECLINED: &str = "declined";
    /// The held call timed out or its conversation expired.
    pub const EXPIRED: &str = "expired";
    /// Dry-run mode: the call was described, not run.
    pub const DRY_RUN: &str = "blocked:dry-run";
    /// A daily cap stopped the call.
    pub const CAP: &str = "blocked:cap";
    /// The call was malformed or named a tool that is not offered.
    pub const INVALID: &str = "blocked:invalid";
    /// An earlier call in the same round was held, so this one was not looked at.
    pub const SKIPPED: &str = "blocked:pending";
    /// The user cancelled the turn: the call was dropped, or a held call was discarded (REQ-AST-022).
    pub const CANCELLED: &str = "cancelled";
    /// An announcement the endpoint's filter dropped (REQ-INB-024).
    pub const FILTERED: &str = "blocked:filtered";
    /// A turn limit, the deadline or a failure streak stopped the turn before this call (REQ-AST-019).
    pub const STOPPED: &str = "blocked:limit";
}

/// A result longer than this many characters also keeps its text in the record's `detail` (REQ-AST-024).
pub const LONG_RESULT_CHARS: usize = 400;
/// The most characters of a long result kept in `detail`.
pub const DETAIL_CHARS: usize = 2_000;

/// Limits and switches that act on calls (REQ-SEC-015, REQ-SEC-016).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionPolicy {
    /// Run nothing that is not read-only; describe it and record it instead.
    pub dry_run: bool,
    /// Calls that are not read-only allowed per local day.
    pub max_actions_per_day: u32,
    /// Costly and destructive calls allowed per local day (part of the above).
    pub max_costly_per_day: u32,
}

impl Default for ActionPolicy {
    fn default() -> Self {
        Self {
            dry_run: false,
            max_actions_per_day: 100_000,
            max_costly_per_day: 100_000,
        }
    }
}

/// Where the log lives and how big it may get.
#[derive(Debug, Clone)]
pub struct AuditConfig {
    /// Size of one file before it is rotated.
    pub max_file_bytes: u64,
    /// The caps and the dry-run switch.
    pub policy: ActionPolicy,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            policy: ActionPolicy::default(),
        }
    }
}

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Format version.
    pub v: u32,
    /// Increases by one per record, across restarts.
    pub id: u64,
    /// Unix time in seconds.
    pub ts: Ts,
    /// The local day number the caps count by.
    pub day: i64,
    /// The turn this belongs to (counts up across restarts).
    pub turn: u64,
    /// What started the turn: `voice`, or `event:<source>`.
    pub trigger: String,
    /// The tool (clipped and made printable: the model chose it).
    pub tool: String,
    /// `read_only`, `reversible`, `costly`, `destructive`, or `unknown`.
    pub tier: String,
    /// The arguments with secrets removed and every value clipped.
    pub args: Value,
    /// One of the [`decision`] strings.
    pub decision: String,
    /// For approved, declined and expired records: the id of the `held` or `proposed` record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_id: Option<u64>,
    /// The call was described but not run because of dry-run mode.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// `ok` or `error` for a call that ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The first part of the result, redacted. Text from outside: show it as data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// For a result over [`LONG_RESULT_CHARS`]: more of its text, redacted and cut at [`DETAIL_CHARS`], for
    /// the portal (REQ-AST-024). Text from outside: show it as data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What the language model cost so far in this turn, in US dollars, when the provider reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_cost_usd: Option<f64>,
    /// A short fixed reason (written by the host, never by the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What the session loop reports about one call.
#[derive(Debug, Clone, Copy)]
pub struct Act<'a> {
    /// Tool name as the model gave it.
    pub tool: &'a str,
    /// Its tier, if the call got far enough to have one.
    pub tier: Option<Tier>,
    /// Its arguments (redacted here).
    pub args: &'a Value,
    /// One of the [`decision`] strings.
    pub decision: &'a str,
    /// The record this one resolves.
    pub held_id: Option<u64>,
    /// How the call ended, when it ran: `Ok` result text or `Err` message.
    pub outcome: Option<Result<&'a str, &'a str>>,
    /// A fixed reason.
    pub note: Option<&'a str>,
}

/// A cap that was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapHit {
    /// Too many actions today.
    Actions(u32),
    /// Too many costly or destructive actions today.
    Costly(u32),
}

impl std::fmt::Display for CapHit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Actions(n) => write!(f, "the limit of {n} actions a day is used up"),
            Self::Costly(n) => write!(
                f,
                "the limit of {n} costly or risky actions a day is used up"
            ),
        }
    }
}

/// What to do with a call that is about to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Go ahead.
    Run,
    /// Dry-run mode: describe, do not run.
    DryRun,
    /// A cap is used up.
    Capped(CapHit),
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    day: i64,
    known: bool,
    actions: u32,
    costly: u32,
}

#[derive(Debug)]
enum Sink {
    Disk {
        path: PathBuf,
        old: PathBuf,
        size: u64,
        /// The file ends in a half-written line: start the next record on a fresh line.
        needs_newline: bool,
    },
    Memory {
        lines: VecDeque<String>,
        bytes: u64,
    },
}

#[derive(Debug)]
struct State {
    sink: Sink,
    max_file_bytes: u64,
    policy: ActionPolicy,
    next_id: u64,
    next_turn: u64,
    counts: Counts,
    /// Damaged lines seen when the log was last read.
    corrupt_lines: u64,
    /// Records that could not be written.
    write_errors: u64,
}

/// The audit log. Cheap to share: wrap it in an [`Arc`].
#[derive(Debug)]
pub struct AuditLog {
    state: Mutex<State>,
}

/// The log's own health, for the endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    /// Bytes in the current file.
    pub bytes: u64,
    /// Size at which the file rotates.
    pub max_file_bytes: u64,
    /// Damaged lines found when reading.
    pub corrupt_lines: u64,
    /// Records that could not be written (disk full, permissions).
    pub write_errors: u64,
}

/// What the read endpoint asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// Records per page (default [`DEFAULT_PAGE`], at most [`MAX_PAGE`]).
    pub limit: Option<usize>,
    /// Only records older than this id (the previous page's `next_before`).
    pub before: Option<u64>,
    /// Only this tool.
    pub tool: Option<String>,
    /// Only this tier.
    pub tier: Option<String>,
    /// Only this decision, or a prefix ending in `:` such as `blocked:`.
    pub decision: Option<String>,
    /// Only triggers starting with this (`voice`, `event`, `event:nas`).
    pub trigger: Option<String>,
    /// Only records at or after this Unix time.
    pub since: Option<Ts>,
}

/// One page of records, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// The records.
    pub records: Vec<Record>,
    /// Pass as `before` to get the next, older page; `None` on the last page.
    pub next_before: Option<u64>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AuditLog {
    /// A log that lives only in memory (tests and a core with no data directory). Bounded like the file.
    #[must_use]
    pub fn in_memory(cfg: AuditConfig) -> Self {
        Self {
            state: Mutex::new(State {
                sink: Sink::Memory {
                    lines: VecDeque::new(),
                    bytes: 0,
                },
                max_file_bytes: cfg.max_file_bytes,
                policy: cfg.policy,
                next_id: 1,
                next_turn: 1,
                counts: Counts::default(),
                corrupt_lines: 0,
                write_errors: 0,
            }),
        }
    }

    /// Open (or start) the log in `dir`. Damaged or half-written lines are skipped; only an unusable
    /// directory is an error.
    ///
    /// # Errors
    /// The directory cannot be created or the log files cannot be inspected.
    pub fn open(dir: &Path, cfg: AuditConfig) -> io::Result<Self> {
        create_private_dir(dir)?;
        let path = dir.join("audit.jsonl");
        let old = dir.join("audit.1.jsonl");
        let mut max_id = 0;
        let mut max_turn = 0;
        let mut corrupt = 0;
        for p in [&old, &path] {
            let Ok(file) = fs::File::open(p) else {
                continue;
            };
            each_file_line(file, |line| match parse_line(line) {
                Some(r) => {
                    max_id = max_id.max(r.id);
                    max_turn = max_turn.max(r.turn);
                }
                None => corrupt += 1,
            });
        }
        let size = fs::metadata(&path).map_or(0, |m| m.len());
        let needs_newline = size > 0 && !ends_with_newline(&path);
        #[cfg(unix)]
        for p in [&path, &old] {
            // A file left by an older version or by hand: make sure only the service user can read it.
            use std::os::unix::fs::PermissionsExt;
            if p.is_file() {
                let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o600));
            }
        }
        Ok(Self {
            state: Mutex::new(State {
                sink: Sink::Disk {
                    path,
                    old,
                    size,
                    needs_newline,
                },
                max_file_bytes: cfg.max_file_bytes,
                policy: cfg.policy,
                next_id: max_id.saturating_add(1),
                next_turn: max_turn.saturating_add(1),
                counts: Counts::default(),
                corrupt_lines: corrupt,
                write_errors: 0,
            }),
        })
    }

    /// The caps and the dry-run switch in force.
    #[must_use]
    pub fn policy(&self) -> ActionPolicy {
        lock(&self.state).policy
    }

    /// Change the caps and the dry-run switch (live).
    pub fn set_policy(&self, policy: ActionPolicy) {
        lock(&self.state).policy = policy;
    }

    /// A new turn id.
    #[must_use]
    pub fn next_turn(&self) -> u64 {
        let mut s = lock(&self.state);
        let t = s.next_turn;
        s.next_turn = t.saturating_add(1);
        t
    }

    /// What may happen to a call of this tier today (REQ-SEC-015, REQ-SEC-016). Read-only calls always run.
    /// Dry-run is checked first: a described call uses none of the day's allowance.
    #[must_use]
    pub fn gate(&self, day: i64, tier: Tier) -> Gate {
        if tier == Tier::ReadOnly {
            return Gate::Run;
        }
        let mut s = lock(&self.state);
        if s.policy.dry_run {
            return Gate::DryRun;
        }
        s.load_counts(day);
        if s.counts.actions >= s.policy.max_actions_per_day {
            return Gate::Capped(CapHit::Actions(s.policy.max_actions_per_day));
        }
        if tier >= Tier::Costly && s.counts.costly >= s.policy.max_costly_per_day {
            return Gate::Capped(CapHit::Costly(s.policy.max_costly_per_day));
        }
        Gate::Run
    }

    /// Actions and costly actions counted today.
    #[must_use]
    pub fn today(&self, day: i64) -> (u32, u32) {
        let mut s = lock(&self.state);
        s.load_counts(day);
        (s.counts.actions, s.counts.costly)
    }

    /// Append one record. Returns its id, or `None` if it could not be written (counted in the health
    /// figures and reported on stderr once per failure, never panics).
    pub fn record(
        &self,
        ts: Ts,
        day: i64,
        turn: u64,
        trigger: &str,
        cost: Option<f64>,
        a: &Act<'_>,
    ) -> Option<u64> {
        let mut s = lock(&self.state);
        let id = s.next_id;
        let (status, result) = match a.outcome {
            None => (None, None),
            Some(Ok(text)) => (Some("ok"), Some(excerpt(text))),
            Some(Err(text)) => (Some("error"), Some(excerpt(text))),
        };
        let detail = match a.outcome {
            Some(Ok(text)) if text.chars().count() > LONG_RESULT_CHARS => Some(redact_detail(text)),
            _ => None,
        };
        let mut rec = Record {
            v: FORMAT,
            id,
            ts,
            day,
            turn,
            trigger: clean_name(trigger),
            tool: clean_name(a.tool),
            tier: a.tier.map_or("unknown", Tier::label).to_owned(),
            args: redact_args(a.args),
            decision: a.decision.to_owned(),
            held_id: a.held_id,
            dry_run: a.decision == decision::DRY_RUN,
            status: status.map(str::to_owned),
            result,
            detail,
            turn_cost_usd: cost.filter(|c| c.is_finite() && *c > 0.0),
            note: a.note.map(|n| clip(n, MAX_VALUE_CHARS)),
        };
        let mut line = serde_json::to_string(&rec).ok()?;
        if line.len() > MAX_LINE_BYTES && rec.detail.is_some() {
            // Multi-byte text can make a long detail too big for one line: keep less rather than lose the record.
            rec.detail = rec.detail.take().map(|d| clip(&d, 500));
            line = serde_json::to_string(&rec).ok()?;
            if line.len() > MAX_LINE_BYTES {
                rec.detail = None;
                line = serde_json::to_string(&rec).ok()?;
            }
        }
        if line.len() > MAX_LINE_BYTES {
            // Cannot happen with the clips above; refuse rather than write something unbounded.
            return None;
        }
        line.push('\n');
        // The daily caps count what ran, whether or not the record could be written: a full or read-only disk
        // must not turn the caps off (REQ-SEC-016).
        let counted = a.decision == decision::RAN || a.decision == decision::APPROVED;
        if counted && a.tier.is_some_and(|t| t != Tier::ReadOnly) {
            s.load_counts(day);
            s.counts.actions = s.counts.actions.saturating_add(1);
            if a.tier >= Some(Tier::Costly) {
                s.counts.costly = s.counts.costly.saturating_add(1);
            }
        }
        if let Err(e) = s.write(&line) {
            s.write_errors = s.write_errors.saturating_add(1);
            crate::diag::error(&format!("could not write the audit log: {e}"));
            return None;
        }
        s.next_id = id.saturating_add(1);
        Some(id)
    }

    /// The log's health figures.
    #[must_use]
    pub fn health(&self) -> Health {
        let s = lock(&self.state);
        Health {
            bytes: match &s.sink {
                Sink::Disk { size, .. } => *size,
                Sink::Memory { bytes, .. } => *bytes,
            },
            max_file_bytes: s.max_file_bytes,
            corrupt_lines: s.corrupt_lines,
            write_errors: s.write_errors,
        }
    }

    /// What the log holds, for diagnostics: how many records, by decision and by tier, the time span, and the
    /// last few calls that failed (tool and time only; no arguments or result text). REQ-CFG-063.
    #[must_use]
    pub fn summary(&self) -> Value {
        let source = lock(&self.state).source();
        let mut records = 0_usize;
        let mut by_decision = std::collections::BTreeMap::<String, u64>::new();
        let mut by_tier = std::collections::BTreeMap::<String, u64>::new();
        // The record with the highest id is the newest and the one with the lowest the oldest; among equal ids
        // the first in the file is the newest and the last the oldest, as a stable sort by id would give.
        let mut newest: Option<(u64, Ts)> = None;
        let mut oldest: Option<(u64, Ts)> = None;
        // (id, ts, tool, decision) of every failed call; the ten with the highest ids are kept at the end.
        let mut failures: Vec<(u64, Ts, String, String)> = Vec::new();
        source.each_line(|line| {
            let Some(r) = parse_line(line) else { return };
            records += 1;
            *by_decision.entry(r.decision.clone()).or_default() += 1;
            *by_tier.entry(r.tier.clone()).or_default() += 1;
            if newest.is_none_or(|(id, _)| r.id > id) {
                newest = Some((r.id, r.ts));
            }
            if oldest.is_none_or(|(id, _)| r.id <= id) {
                oldest = Some((r.id, r.ts));
            }
            if r.status.as_deref() == Some("error") {
                failures.push((r.id, r.ts, r.tool, r.decision));
            }
        });
        failures.sort_by_key(|f| std::cmp::Reverse(f.0));
        let failed: Vec<Value> = failures
            .into_iter()
            .take(10)
            .map(|(_, ts, tool, decision)| json!({ "ts": ts, "tool": tool, "decision": decision }))
            .collect();
        json!({
            "records": records, "by_decision": by_decision, "by_tier": by_tier,
            "newest": newest.map(|n| n.1), "oldest": oldest.map(|o| o.1),
            "recent_failures": failed,
        })
    }

    /// Records matching `q`, newest first.
    #[must_use]
    pub fn query(&self, q: &Query) -> Page {
        let limit = q.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
        // Records are appended in id order, so the newest are at the end of the newest file: read from there
        // and stop as soon as the page is full and one more match proves there is a next page. If an id is
        // ever not lower than the one before it (a hand-edited file), fall back to reading everything.
        let source = lock(&self.state).source();
        let mut records: Vec<Record> = Vec::new();
        let mut more = false;
        let mut ordered = true;
        let mut bad = 0_u64;
        let mut stopped = false;
        let mut last_id = u64::MAX;
        source.each_line_rev(|line| {
            let Some(r) = parse_line(line) else {
                bad += 1;
                return true;
            };
            if r.id >= last_id {
                ordered = false;
                return false;
            }
            last_id = r.id;
            if !q.matches(&r) {
                return true;
            }
            if records.len() >= limit {
                more = true;
                stopped = true;
                return false;
            }
            records.push(r);
            true
        });
        if !ordered {
            return self.query_sorted(q, limit);
        }
        if !stopped {
            // The whole log was read, so the damaged-line figure is current.
            lock(&self.state).corrupt_lines = bad;
        }
        let next_before = if more {
            records.last().map(|r| r.id)
        } else {
            None
        };
        Page {
            records,
            next_before,
        }
    }

    /// [`AuditLog::query`] for a log whose ids are not in order: read everything, sort, drop repeated ids.
    fn query_sorted(&self, q: &Query, limit: usize) -> Page {
        let source = lock(&self.state).source();
        let mut all: Vec<Record> = Vec::new();
        let mut bad = 0_u64;
        source.each_line(|line| match parse_line(line) {
            Some(r) => all.push(r),
            None => bad += 1,
        });
        lock(&self.state).corrupt_lines = bad;
        all.sort_by_key(|r| std::cmp::Reverse(r.id));
        all.dedup_by_key(|r| r.id);
        let mut matched = all.into_iter().filter(|r| q.matches(r));
        let records: Vec<Record> = matched.by_ref().take(limit).collect();
        let next_before = if matched.next().is_some() {
            records.last().map(|r| r.id)
        } else {
            None
        };
        Page {
            records,
            next_before,
        }
    }
}

impl Query {
    fn matches(&self, r: &Record) -> bool {
        self.before.is_none_or(|b| r.id < b)
            && self.tool.as_ref().is_none_or(|t| &r.tool == t)
            && self.tier.as_ref().is_none_or(|t| &r.tier == t)
            && self.decision.as_ref().is_none_or(|d| {
                if d.ends_with(':') {
                    r.decision.starts_with(d.as_str())
                } else {
                    &r.decision == d
                }
            })
            && self
                .trigger
                .as_ref()
                .is_none_or(|t| r.trigger == *t || r.trigger.starts_with(&format!("{t}:")))
            && self.since.is_none_or(|t| r.ts >= t)
    }
}

impl State {
    fn write(&mut self, line: &str) -> io::Result<()> {
        let max = self.max_file_bytes;
        let len = line.len() as u64;
        match &mut self.sink {
            Sink::Memory { lines, bytes } => {
                lines.push_back(line.to_owned());
                *bytes += len;
                // Same bound as the disk: the current file plus one rotated file.
                while *bytes > max.saturating_mul(2) && lines.len() > 1 {
                    if let Some(l) = lines.pop_front() {
                        *bytes = bytes.saturating_sub(l.len() as u64);
                    }
                }
                Ok(())
            }
            Sink::Disk {
                path,
                old,
                size,
                needs_newline,
            } => {
                if *size > 0 && *size + len > max {
                    // Rotate: the previous rotated file is dropped, the current one becomes it.
                    let _ = fs::remove_file(&*old);
                    fs::rename(&*path, &*old)?;
                    *size = 0;
                    *needs_newline = false;
                }
                let mut f = open_append(path)?;
                if *needs_newline {
                    f.write_all(b"\n")?;
                    *size += 1;
                    *needs_newline = false;
                }
                f.write_all(line.as_bytes())?;
                *size += len;
                Ok(())
            }
        }
    }

    /// What a read of the log works from. Taken under the lock; the reading itself needs no lock, because open
    /// file handles keep pointing at the file they were opened on even if it is rotated meanwhile.
    fn source(&self) -> Source {
        match &self.sink {
            Sink::Memory { lines, .. } => Source::Memory(lines.iter().cloned().collect()),
            Sink::Disk { path, old, .. } => Source::Files {
                old: fs::File::open(old).ok(),
                current: fs::File::open(path).ok(),
            },
        }
    }

    /// Make the day's counters match the log (it is the record of what ran), the first time a day is seen.
    /// After that [`AuditLog::record`] keeps them up to date, so this does not run again for the same day.
    fn load_counts(&mut self, day: i64) {
        if self.counts.known && self.counts.day == day {
            return;
        }
        let mut c = Counts {
            day,
            known: true,
            actions: 0,
            costly: 0,
        };
        self.source().each_line(|line| {
            let Some(r) = parse_line(line) else { return };
            if r.day != day {
                return;
            }
            if r.decision == decision::RAN || r.decision == decision::APPROVED {
                if r.tier != Tier::ReadOnly.label() && r.tier != "unknown" {
                    c.actions = c.actions.saturating_add(1);
                }
                if r.tier == Tier::Costly.label() || r.tier == Tier::Destructive.label() {
                    c.costly = c.costly.saturating_add(1);
                }
            }
        });
        self.counts = c;
    }
}

/// The log as lines, ready to read without the lock.
enum Source {
    /// The in-memory log, oldest line first.
    Memory(Vec<String>),
    /// The rotated file and the current one (either may be missing).
    Files {
        old: Option<fs::File>,
        current: Option<fs::File>,
    },
}

impl Source {
    /// Every non-blank line, oldest first.
    fn each_line(self, mut f: impl FnMut(&str)) {
        match self {
            Self::Memory(lines) => {
                for l in lines.iter().filter(|l| !l.trim().is_empty()) {
                    f(l);
                }
            }
            Self::Files { old, current } => {
                for file in [old, current].into_iter().flatten() {
                    each_file_line(file, &mut f);
                }
            }
        }
    }

    /// Non-blank lines, newest first, until `f` returns false.
    fn each_line_rev(self, mut f: impl FnMut(&str) -> bool) {
        match self {
            Self::Memory(lines) => {
                for l in lines.iter().rev().filter(|l| !l.trim().is_empty()) {
                    if !f(l) {
                        return;
                    }
                }
            }
            Self::Files { old, current } => {
                for file in [current, old].into_iter().flatten() {
                    let mut lines = RevLines::new(file, REV_CHUNK);
                    while let Some(l) = lines.next_line() {
                        let text = String::from_utf8_lossy(&l);
                        if !text.trim().is_empty() && !f(&text) {
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// Call `f` with each non-blank line of a file, oldest first, without holding the whole file.
fn each_file_line(file: fs::File, mut f: impl FnMut(&str)) {
    use std::io::BufRead;
    let mut reader = io::BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                if !text.trim().is_empty() {
                    f(&text);
                }
            }
        }
    }
}

/// How much of the end of a file is read at a time when reading newest first.
const REV_CHUNK: usize = 4096;

/// The lines of a file from the last to the first, reading it backwards in chunks.
struct RevLines {
    file: fs::File,
    chunk: usize,
    /// Bytes before this offset have not been read yet.
    pos: u64,
    /// The start of a line whose beginning is in an earlier chunk.
    carry: Vec<u8>,
    /// Complete lines read but not yet handed out, in file order.
    ready: Vec<Vec<u8>>,
}

impl RevLines {
    fn new(mut file: fs::File, chunk: usize) -> Self {
        use std::io::{Seek, SeekFrom};
        let pos = file.seek(SeekFrom::End(0)).unwrap_or(0);
        Self {
            file,
            chunk: chunk.max(1),
            pos,
            carry: Vec::new(),
            ready: Vec::new(),
        }
    }

    /// The next line towards the start of the file, without its newline.
    fn next_line(&mut self) -> Option<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        loop {
            if let Some(line) = self.ready.pop() {
                return Some(line);
            }
            if self.pos == 0 {
                return (!self.carry.is_empty()).then(|| std::mem::take(&mut self.carry));
            }
            let n = u64::try_from(self.chunk).map_or(self.pos, |c| c.min(self.pos));
            self.pos -= n;
            let mut data = vec![0_u8; usize::try_from(n).unwrap_or(self.chunk)];
            if self.file.seek(SeekFrom::Start(self.pos)).is_err()
                || self.file.read_exact(&mut data).is_err()
            {
                self.pos = 0;
                self.carry.clear();
                return None;
            }
            data.extend_from_slice(&self.carry);
            let mut parts = data.split(|b| *b == b'\n').map(<[u8]>::to_vec);
            // The first part may continue into the chunk before; every later one is a whole line.
            self.carry = parts.next().unwrap_or_default();
            self.ready = parts.collect();
        }
    }
}

fn parse_line(line: &str) -> Option<Record> {
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    serde_json::from_str::<Record>(line)
        .ok()
        .filter(|r| r.v >= 1 && r.v <= FORMAT)
}

fn ends_with_newline(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = fs::File::open(path) else {
        return true;
    };
    let mut b = [0u8; 1];
    f.seek(SeekFrom::End(-1))
        .and_then(|_| f.read_exact(&mut b))
        .map_or(true, |()| b[0] == b'\n')
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

fn open_append(path: &Path) -> io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

// ----- making text safe to keep -----

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}...")
}

/// A long result for the portal: cut at [`DETAIL_CHARS`], secrets removed line by line, line breaks kept so it
/// stays readable.
fn redact_detail(text: &str) -> String {
    let cut: String = text.chars().take(DETAIL_CHARS + 1).collect();
    let lines: Vec<String> = cut
        .split('\n')
        .map(|l| redact_words(&printable(l)))
        .collect();
    clip(&lines.join("\n"), DETAIL_CHARS)
}

fn printable(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A tool name, trigger or key: only plain characters, clipped. The model and event sources choose these.
fn clean_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':') {
                c
            } else {
                '_'
            }
        })
        .collect();
    clip(&cleaned, MAX_NAME_CHARS)
}

/// Whether a key names something secret.
fn sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "pwd",
        "passphrase",
        "key",
        "auth",
        "bearer",
        "cookie",
        "credential",
        "private",
        "signature",
        "session",
        "otp",
    ]
    .iter()
    .any(|s| k.contains(s))
}

/// Start of well-known credential formats.
fn secret_prefix(w: &str) -> bool {
    const PREFIXES: [&str; 14] = [
        "sk-",
        "sk_",
        "pk_",
        "rk_",
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "AIza",
        "cdt_",
    ];
    (w.len() >= 8 && PREFIXES.iter().any(|p| w.starts_with(p)))
        || (w.len() >= 20 && w.starts_with("eyJ"))
}

/// A long run of token-like characters that mixes letters and digits.
fn looks_like_token(w: &str) -> bool {
    w.len() >= 32
        && !w.starts_with('/')
        && !w.contains("://")
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '=' | '.'))
        && w.chars().any(|c| c.is_ascii_digit())
        && w.chars().any(|c| c.is_ascii_alphabetic())
}

/// Replace the secrets in free text: bearer values, `key=value` pairs with a secret-sounding key, password
/// parts of URLs, well-known token formats, and long token-like strings.
#[must_use]
pub fn redact_text(text: &str) -> String {
    redact_words(&printable(&clip(text, 4 * MAX_VALUE_CHARS)))
}

fn redact_words(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut after_scheme = false;
    for word in text.split(' ') {
        let bare = word.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
            )
        });
        let lower = bare.to_ascii_lowercase();
        let replaced = if after_scheme && !bare.is_empty() {
            REDACTED.to_owned()
        } else {
            redact_word(word, bare)
        };
        after_scheme = matches!(
            lower.trim_end_matches(':'),
            "bearer" | "basic" | "token" | "authorization" | "apikey" | "password" | "secret"
        );
        out.push(replaced);
    }
    out.join(" ")
}

fn redact_word(word: &str, bare: &str) -> String {
    if bare.is_empty() {
        return word.to_owned();
    }
    // user:password@host in a URL.
    if let Some(scheme_end) = bare.find("://") {
        let rest = &bare[scheme_end + 3..];
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if let Some(at) = authority.rfind('@') {
            let tail = &rest[at + 1..];
            let head = &bare[..scheme_end + 3];
            return redact_query(&format!("{head}{REDACTED}@{tail}"));
        }
        return redact_query(word);
    }
    if bare.contains('?') || bare.contains('&') {
        return redact_query(word);
    }
    // key=value or key:value with a secret-sounding key.
    for sep in ['=', ':'] {
        if let Some((k, v)) = bare.split_once(sep) {
            if !v.is_empty() && sensitive_key(k.trim_matches(|c: char| !c.is_alphanumeric())) {
                return format!("{k}{sep}{REDACTED}");
            }
        }
    }
    if secret_prefix(bare) || looks_like_token(bare) {
        return word.replace(bare, REDACTED);
    }
    word.to_owned()
}

/// Redact `name=value` pairs in a query string.
fn redact_query(s: &str) -> String {
    let Some(q) = s.find('?') else {
        return s.to_owned();
    };
    let (head, query) = s.split_at(q + 1);
    let parts: Vec<String> = query
        .split('&')
        .map(|p| match p.split_once('=') {
            Some((k, v)) if !v.is_empty() && (sensitive_key(k) || looks_like_token(v)) => {
                format!("{k}={REDACTED}")
            }
            _ => p.to_owned(),
        })
        .collect();
    format!("{head}{}", parts.join("&"))
}

/// A result or message shortened for the log.
fn excerpt(text: &str) -> String {
    clip(&redact_text(text), MAX_VALUE_CHARS)
}

fn redact_value(v: &Value, depth: usize) -> Value {
    match v {
        Value::String(s) => Value::String(clip(&redact_text(s), MAX_VALUE_CHARS)),
        Value::Array(a) => {
            if depth >= 4 {
                return json!("...");
            }
            Value::Array(
                a.iter()
                    .take(20)
                    .map(|x| redact_value(x, depth + 1))
                    .collect(),
            )
        }
        Value::Object(m) => {
            if depth >= 4 {
                return json!("...");
            }
            let mut out = Map::new();
            for (k, x) in m.iter().take(30) {
                let value = if sensitive_key(k) {
                    json!(REDACTED)
                } else {
                    redact_value(x, depth + 1)
                };
                out.insert(clean_name(k), value);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Strings longer than this are spoken as "a long value" instead of being read out.
pub const MAX_SPOKEN_VALUE_CHARS: usize = 60;

/// Arguments as they are read aloud when a call is held (REQ-SEC-011): the same redaction as the audit log
/// (`redact_value`), and any text value longer than [`MAX_SPOKEN_VALUE_CHARS`] replaced by "a long value".
/// For speech only: the stored action keeps its exact arguments.
#[must_use]
pub fn spoken_args(args: &Value) -> Value {
    fn shorten(v: Value) -> Value {
        match v {
            Value::String(s) if s.chars().count() > MAX_SPOKEN_VALUE_CHARS => {
                json!("a long value")
            }
            Value::Array(a) => Value::Array(a.into_iter().map(shorten).collect()),
            Value::Object(m) => {
                Value::Object(m.into_iter().map(|(k, x)| (k, shorten(x))).collect())
            }
            other => other,
        }
    }
    shorten(redact_value(args, 0))
}

/// Arguments as they are kept: secrets replaced, values clipped, the whole bounded.
#[must_use]
pub fn redact_args(args: &Value) -> Value {
    let v = redact_value(args, 0);
    let text = v.to_string();
    if text.len() <= MAX_ARGS_BYTES {
        return v;
    }
    let cut: String = text.chars().take(MAX_ARGS_BYTES / 2).collect();
    json!({ "_truncated": cut })
}

// ----- the session loop's view -----

/// One turn's handle on the log: carries the ids, the day and the clock so the loop does not have to.
#[derive(Debug, Clone)]
pub struct TurnLog {
    log: Arc<AuditLog>,
    turn: u64,
    trigger: String,
    day: i64,
    now: Ts,
    spend_at_start: f64,
}

impl TurnLog {
    /// Start recording a turn. `trigger` is `voice` or `event:<source>`; `spend_now` is the day's spend so
    /// far, so records can say what the turn cost.
    #[must_use]
    pub fn begin(log: &Arc<AuditLog>, trigger: &str, day: i64, now: Ts, spend_now: f64) -> Self {
        Self {
            log: Arc::clone(log),
            turn: log.next_turn(),
            trigger: trigger.to_owned(),
            day,
            now,
            spend_at_start: spend_now,
        }
    }

    /// Record one call. `spend_now` is the day's spend at this moment.
    pub fn record(&self, spend_now: f64, a: &Act<'_>) -> Option<u64> {
        let cost = spend_now - self.spend_at_start;
        self.log.record(
            self.now,
            self.day,
            self.turn,
            &self.trigger,
            (cost > 0.0).then_some(cost),
            a,
        )
    }

    /// May this call run?
    #[must_use]
    pub fn gate(&self, tier: Tier) -> Gate {
        self.log.gate(self.day, tier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;

    fn act<'a>(tool: &'a str, tier: Tier, args: &'a Value, d: &'a str) -> Act<'a> {
        Act {
            tool,
            tier: Some(tier),
            args,
            decision: d,
            held_id: None,
            outcome: None,
            note: None,
        }
    }

    fn cfg(max: u64) -> AuditConfig {
        AuditConfig {
            max_file_bytes: max,
            ..AuditConfig::default()
        }
    }

    // REQ-SEC-014
    #[test]
    fn secrets_in_arguments_and_results_are_redacted() {
        let args = json!({
            "url": "https://bob:hunter2@example.com/x?api_key=abc123&room=den",
            "Authorization": "Bearer abcdefghijklmnop",
            "note": "use Bearer sk-live-1234567890abcdef please",
            "password": "p",
            "nested": {"client_secret": "zzz", "keep": "plain words"},
            "blob": "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8",
            "n": 5,
        });
        let v = redact_args(&args);
        let s = v.to_string();
        for leak in [
            "hunter2",
            "abc123",
            "abcdefghijklmnop",
            "sk-live",
            "zzz",
            "A1b2C3d4E5f6",
        ] {
            assert!(!s.contains(leak), "{leak} leaked in {s}");
        }
        assert!(s.contains("room=den"), "{s}");
        assert!(s.contains("plain words"), "{s}");
        assert_eq!(v["n"], 5);
        let r = excerpt("token=abc and cdt_0123456789abcdef and password: hunter2");
        assert!(!r.contains("abc") || r.contains(REDACTED));
        assert!(!r.contains("hunter2") && !r.contains("cdt_0123"), "{r}");
    }

    // REQ-SEC-011, REQ-SEC-014
    #[test]
    fn spoken_arguments_hide_secrets_and_shorten_long_values() {
        let args = json!({
            "note": "use Bearer abcdefghijklmnop1234",
            "api_key": "sk-live-0123456789abcdef",
            "url": "https://bob:hunter2@example.com/x",
            "body": "word ".repeat(40),
            "room": "den",
        });
        let s = spoken_args(&args).to_string();
        for leak in ["abcdefghijklmnop1234", "sk-live", "hunter2", "word word"] {
            assert!(!s.contains(leak), "{leak} leaked in {s}");
        }
        assert!(s.contains("a long value") && s.contains("den"), "{s}");
    }

    // REQ-SEC-014
    #[test]
    fn values_are_clipped_and_cleaned() {
        let long = "x ".repeat(500);
        let args = json!({ "text": long, "weird\nkey": "a\u{7}b\nc", "list": (0..100).collect::<Vec<_>>() });
        let v = redact_args(&args);
        assert!(v.to_string().len() <= MAX_ARGS_BYTES + 50, "{v}");
        let big = redact_args(
            &json!({"a": "y".repeat(190), "b": "z".repeat(190), "c": "q".repeat(190),
            "d": "w".repeat(190), "e": "r".repeat(190), "f": "t".repeat(190)}),
        );
        assert!(big.get("_truncated").is_some(), "{big}");
        let r = excerpt(&"é".repeat(1000));
        assert!(r.chars().count() <= MAX_VALUE_CHARS + 3);
        assert_eq!(clean_name("rm -rf /;\n"), "rm_-rf____");
        assert!(clean_name(&"a".repeat(500)).len() <= MAX_NAME_CHARS + 3);
    }

    // REQ-SEC-014
    #[test]
    fn records_are_appended_in_a_private_file_and_read_back() {
        let dir = TempDir::new("audit");
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        let args = json!({"minutes": 10});
        let t = log.next_turn();
        let a = Act {
            outcome: Some(Ok("Timer set.")),
            ..act("set_timer", Tier::Reversible, &args, decision::RAN)
        };
        let id = log.record(1000, 7, t, "voice", Some(0.002), &a).unwrap();
        let held = log
            .record(
                1001,
                7,
                t,
                "event:nas-monitor",
                None,
                &act("docker__restart", Tier::Costly, &json!({}), decision::HELD),
            )
            .unwrap();
        assert_eq!((id, held), (1, 2));
        let page = log.query(&Query::default());
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.records[0].id, 2);
        let r = &page.records[1];
        assert_eq!(
            (r.tool.as_str(), r.tier.as_str(), r.status.as_deref()),
            ("set_timer", "reversible", Some("ok"))
        );
        assert_eq!(r.turn_cost_usd, Some(0.002));
        assert_eq!(r.trigger, "voice");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.0.join("audit.jsonl"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // ids and turns continue after a restart
        drop(log);
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        assert_eq!(log.next_turn(), t + 1);
        let again = log
            .record(
                1002,
                7,
                t + 1,
                "voice",
                None,
                &act("set_timer", Tier::Reversible, &args, decision::RAN),
            )
            .unwrap();
        assert_eq!(again, 3);
    }

    // REQ-SEC-014
    #[test]
    fn the_file_rotates_and_stays_bounded() {
        let dir = TempDir::new("audit-rot");
        let log = AuditLog::open(&dir.0, cfg(2_000)).unwrap();
        let args = json!({"n": 1});
        for i in 0..200 {
            log.record(
                i,
                1,
                1,
                "voice",
                None,
                &act("set_timer", Tier::Reversible, &args, decision::RAN),
            );
        }
        let size = |n: &str| fs::metadata(dir.0.join(n)).map_or(0, |m| m.len());
        assert!(size("audit.jsonl") <= 2_000, "{}", size("audit.jsonl"));
        assert!(size("audit.1.jsonl") <= 2_000);
        assert!(size("audit.1.jsonl") > 0, "it rotated");
        let all = log.query(&Query {
            limit: Some(MAX_PAGE),
            ..Query::default()
        });
        assert!(!all.records.is_empty() && all.records.len() < 200);
        assert_eq!(all.records[0].id, 200, "the newest record survives");
        let ids: Vec<u64> = all.records.iter().map(|r| r.id).collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1] + 1), "no gaps: {ids:?}");
        assert!(log.health().bytes <= 2_000);
    }

    // REQ-SEC-014
    #[test]
    fn a_damaged_or_half_written_log_is_tolerated_at_start_up() {
        let dir = TempDir::new("audit-bad");
        {
            let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
            for _ in 0..3 {
                log.record(
                    5,
                    1,
                    4,
                    "voice",
                    None,
                    &act("set_timer", Tier::Reversible, &json!({}), decision::RAN),
                );
            }
        }
        // garbage in the middle, a line from the future, and a crash mid-write at the end
        let path = dir.0.join("audit.jsonl");
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str(
            "{not json\n\u{0}\u{1}binary\n{\"v\":99,\"id\":1}\n{\"v\":1,\"id\":9,\"ts\":",
        );
        fs::write(&path, text).unwrap();
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        let h = log.health();
        assert!(h.corrupt_lines >= 3, "{h:?}");
        let id = log
            .record(
                6,
                1,
                5,
                "voice",
                None,
                &act("set_timer", Tier::Reversible, &json!({}), decision::RAN),
            )
            .unwrap();
        assert_eq!(id, 4, "continues after the last good record");
        let page = log.query(&Query::default());
        assert_eq!(page.records.len(), 4);
        // the new record starts on its own line, so it is readable
        assert_eq!(page.records[0].id, 4);
        // an unreadable file (a directory in its place) is an error to report, never a panic
        let bad = TempDir::new("audit-dir");
        fs::create_dir_all(bad.0.join("audit.jsonl")).unwrap();
        let log = AuditLog::open(&bad.0, AuditConfig::default()).unwrap();
        assert!(
            log.record(
                1,
                1,
                1,
                "voice",
                None,
                &act("x", Tier::ReadOnly, &json!({}), decision::RAN)
            )
            .is_none()
        );
        assert_eq!(log.health().write_errors, 1);
    }

    // REQ-SEC-014
    #[test]
    fn paging_and_filters() {
        let log = AuditLog::in_memory(AuditConfig::default());
        let none = json!({});
        for i in 0..10_i64 {
            let (tool, tier, d, trig) = if i % 2 == 0 {
                ("set_timer", Tier::Reversible, decision::RAN, "voice")
            } else {
                (
                    "docker__restart",
                    Tier::Costly,
                    decision::PROPOSED,
                    "event:nas",
                )
            };
            log.record(100 + i, 1, 1, trig, None, &act(tool, tier, &none, d));
        }
        let p1 = log.query(&Query {
            limit: Some(4),
            ..Query::default()
        });
        assert_eq!(
            p1.records.iter().map(|r| r.id).collect::<Vec<_>>(),
            [10, 9, 8, 7]
        );
        assert_eq!(p1.next_before, Some(7));
        let p2 = log.query(&Query {
            limit: Some(4),
            before: p1.next_before,
            ..Query::default()
        });
        assert_eq!(p2.records[0].id, 6);
        let p3 = log.query(&Query {
            limit: Some(4),
            before: p2.next_before,
            ..Query::default()
        });
        assert_eq!(p3.records.len(), 2);
        assert_eq!(p3.next_before, None);
        let by = |f: &dyn Fn(&mut Query)| {
            let mut q = Query::default();
            f(&mut q);
            log.query(&q).records.len()
        };
        assert_eq!(by(&|q| q.tool = Some("docker__restart".into())), 5);
        assert_eq!(by(&|q| q.tier = Some("reversible".into())), 5);
        assert_eq!(by(&|q| q.decision = Some("proposed".into())), 5);
        assert_eq!(by(&|q| q.trigger = Some("event".into())), 5);
        assert_eq!(by(&|q| q.trigger = Some("event:nas".into())), 5);
        assert_eq!(by(&|q| q.trigger = Some("eve".into())), 0);
        assert_eq!(by(&|q| q.since = Some(107)), 3);
        assert_eq!(by(&|q| q.decision = Some("blocked:".into())), 0);
        assert_eq!(by(&|q| q.limit = Some(0)), 1, "limit is at least one");
    }

    // REQ-SEC-015
    #[test]
    fn daily_caps_count_what_ran_and_reset_with_the_day() {
        let log = AuditLog::in_memory(AuditConfig {
            policy: ActionPolicy {
                dry_run: false,
                max_actions_per_day: 3,
                max_costly_per_day: 1,
            },
            ..AuditConfig::default()
        });
        let none = json!({});
        let run = |tier, d| {
            log.record(1, 5, 1, "voice", None, &act("t", tier, &none, d));
        };
        assert_eq!(log.gate(5, Tier::Costly), Gate::Run);
        run(Tier::Costly, decision::APPROVED);
        assert_eq!(
            log.gate(5, Tier::Costly),
            Gate::Capped(CapHit::Costly(1)),
            "costly cap"
        );
        assert_eq!(log.gate(5, Tier::Reversible), Gate::Run);
        run(Tier::Reversible, decision::RAN);
        run(Tier::Reversible, decision::RAN);
        assert_eq!(
            log.gate(5, Tier::Reversible),
            Gate::Capped(CapHit::Actions(3))
        );
        // reads, held and blocked records do not count
        assert_eq!(log.gate(5, Tier::ReadOnly), Gate::Run);
        run(Tier::ReadOnly, decision::RAN);
        run(Tier::Costly, decision::HELD);
        assert_eq!(log.today(5), (3, 1));
        // a new day starts clean
        assert_eq!(log.gate(6, Tier::Costly), Gate::Run);
        assert_eq!(log.today(6), (0, 0));
    }

    // REQ-SEC-015
    #[test]
    fn the_day_s_count_survives_a_restart() {
        let dir = TempDir::new("audit-caps");
        let policy = ActionPolicy {
            max_actions_per_day: 2,
            ..ActionPolicy::default()
        };
        let c = AuditConfig {
            policy,
            ..AuditConfig::default()
        };
        {
            let log = AuditLog::open(&dir.0, c.clone()).unwrap();
            for _ in 0..2 {
                log.record(
                    1,
                    9,
                    1,
                    "voice",
                    None,
                    &act("t", Tier::Reversible, &json!({}), decision::RAN),
                );
            }
        }
        let log = AuditLog::open(&dir.0, c).unwrap();
        assert_eq!(
            log.gate(9, Tier::Reversible),
            Gate::Capped(CapHit::Actions(2))
        );
    }

    // REQ-SEC-016
    #[test]
    fn dry_run_gates_everything_but_reads_and_uses_no_allowance() {
        let log = AuditLog::in_memory(AuditConfig::default());
        let mut p = log.policy();
        p.dry_run = true;
        log.set_policy(p);
        assert_eq!(log.gate(1, Tier::ReadOnly), Gate::Run);
        for t in [Tier::Reversible, Tier::Costly, Tier::Destructive] {
            assert_eq!(log.gate(1, t), Gate::DryRun);
        }
        let id = log
            .record(
                1,
                1,
                1,
                "voice",
                None,
                &act("t", Tier::Costly, &json!({}), decision::DRY_RUN),
            )
            .unwrap();
        let page = log.query(&Query::default());
        assert_eq!(page.records[0].id, id);
        assert!(page.records[0].dry_run);
        assert_eq!(log.today(1), (0, 0));
    }

    fn ids(p: &Page) -> Vec<u64> {
        p.records.iter().map(|r| r.id).collect()
    }

    fn put(log: &AuditLog, n: u64) {
        let args = json!({"n": "x".repeat(150)});
        for i in 0..n {
            log.record(
                i64::try_from(i).unwrap(),
                1,
                1,
                "voice",
                None,
                &act("set_timer", Tier::Reversible, &args, decision::RAN),
            );
        }
    }

    // REQ-SEC-014
    #[test]
    fn a_page_that_ends_exactly_at_the_oldest_record_has_no_next_page() {
        let log = AuditLog::in_memory(AuditConfig::default());
        put(&log, 10);
        let page = |limit, before| {
            log.query(&Query {
                limit: Some(limit),
                before,
                ..Query::default()
            })
        };
        let p = page(10, None);
        assert_eq!((p.records.len(), p.next_before), (10, None), "all of it");
        let p = page(9, None);
        assert_eq!((ids(&p).last().copied(), p.next_before), (Some(2), Some(2)));
        let p = page(5, Some(6));
        assert_eq!((ids(&p), p.next_before), (vec![5, 4, 3, 2, 1], None));
        let p = page(4, Some(6));
        assert_eq!((ids(&p), p.next_before), (vec![5, 4, 3, 2], Some(2)));
        let p = page(5, Some(1));
        assert_eq!((p.records.len(), p.next_before), (0, None), "nothing older");
        let p = page(5, Some(100));
        assert_eq!(ids(&p), [10, 9, 8, 7, 6], "a cursor past the newest");
    }

    // REQ-SEC-014
    #[test]
    fn paging_walks_a_rotated_log_on_disk_without_a_gap_or_a_repeat() {
        let dir = TempDir::new("audit-pages");
        // Files several chunks long, so lines straddle the chunk boundaries of the backwards read.
        let log = AuditLog::open(&dir.0, cfg(20_000)).unwrap();
        put(&log, 300);
        assert!(fs::metadata(dir.0.join("audit.1.jsonl")).is_ok(), "rotated");
        assert!(fs::metadata(dir.0.join("audit.1.jsonl")).unwrap().len() > 2 * REV_CHUNK as u64);
        let mut seen = Vec::new();
        let mut before = None;
        loop {
            let p = log.query(&Query {
                limit: Some(37),
                before,
                ..Query::default()
            });
            seen.extend(ids(&p));
            match p.next_before {
                Some(b) => before = Some(b),
                None => break,
            }
        }
        assert_eq!(seen[0], 300);
        assert!(seen.windows(2).all(|w| w[0] == w[1] + 1), "{seen:?}");
        let all = log.query(&Query {
            limit: Some(MAX_PAGE),
            ..Query::default()
        });
        assert_eq!(ids(&all), seen, "one big page and many small ones agree");
    }

    // REQ-SEC-014
    #[test]
    fn a_log_out_of_order_or_with_repeated_ids_is_still_sorted_and_deduplicated() {
        let dir = TempDir::new("audit-order");
        {
            let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
            put(&log, 5);
        }
        let path = dir.0.join("audit.jsonl");
        let lines: Vec<String> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        // 3, 1, 5, 2, 4, then 2 again
        let order = [2, 0, 4, 1, 3, 1];
        let text: String = order.iter().map(|i| format!("{}\n", lines[*i])).collect();
        fs::write(&path, text).unwrap();
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        let p = log.query(&Query {
            limit: Some(3),
            ..Query::default()
        });
        assert_eq!((ids(&p), p.next_before), (vec![5, 4, 3], Some(3)));
        let p = log.query(&Query::default());
        assert_eq!(ids(&p), [5, 4, 3, 2, 1]);
        assert_eq!(log.summary()["records"], 6, "the summary counts lines");
    }

    // REQ-SEC-014
    #[test]
    fn damaged_lines_are_skipped_and_counted_only_by_a_full_read() {
        let dir = TempDir::new("audit-damage");
        {
            let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
            put(&log, 6);
        }
        let path = dir.0.join("audit.jsonl");
        let lines: Vec<String> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let mut text = String::new();
        for (i, l) in lines.iter().enumerate() {
            text.push_str(l);
            text.push('\n');
            if i == 1 || i == 3 {
                text.push_str("{broken\n\n");
            }
        }
        fs::write(&path, text).unwrap();
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        assert_eq!(log.health().corrupt_lines, 2);
        // Damage the count by hand, then see which queries refresh it.
        lock(&log.state).corrupt_lines = 99;
        let p = log.query(&Query {
            limit: Some(2),
            ..Query::default()
        });
        assert_eq!(ids(&p), [6, 5]);
        assert_eq!(log.health().corrupt_lines, 99, "stopped before the damage");
        let p = log.query(&Query::default());
        assert_eq!(ids(&p), [6, 5, 4, 3, 2, 1], "damaged lines are skipped");
        assert_eq!(log.health().corrupt_lines, 2, "a full read recounts them");
    }

    // REQ-CFG-063
    #[test]
    fn the_summary_counts_everything_and_lists_the_latest_failures_first() {
        let dir = TempDir::new("audit-summary");
        let log = AuditLog::open(&dir.0, cfg(3_000)).unwrap();
        let args = json!({});
        for i in 0..40_i64 {
            let outcome = if i % 4 == 0 { Err("boom") } else { Ok("fine") };
            let a = Act {
                outcome: Some(outcome),
                ..act("set_timer", Tier::Reversible, &args, decision::RAN)
            };
            log.record(1_000 + i, 1, 1, "voice", None, &a);
        }
        let s = log.summary();
        let kept = s["records"].as_u64().unwrap();
        assert!(
            kept > 10 && kept < 40,
            "rotation dropped the oldest: {kept}"
        );
        assert_eq!(s["by_decision"]["ran"], kept);
        assert_eq!(s["newest"], 1_039);
        assert_eq!(s["oldest"], 1_040 - kept);
        let failures = s["recent_failures"].as_array().unwrap();
        let times: Vec<i64> = failures.iter().map(|f| f["ts"].as_i64().unwrap()).collect();
        assert_eq!(times[0], 1_036, "newest failure first");
        assert!(times.windows(2).all(|w| w[0] > w[1]) && times.len() <= 10);
    }

    #[test]
    fn lines_are_read_backwards_across_chunk_boundaries() {
        let dir = TempDir::new("audit-rev");
        let path = dir.0.join("f.txt");
        let text = "one\n\ntwo-é-two\nthree\nfour with more text\nlast without newline";
        fs::write(&path, text).unwrap();
        for chunk in [1, 2, 3, 5, 8, 4096] {
            let mut rev = RevLines::new(fs::File::open(&path).unwrap(), chunk);
            let mut got = Vec::new();
            while let Some(l) = rev.next_line() {
                got.push(String::from_utf8(l).unwrap());
            }
            let mut want: Vec<String> = text.split('\n').map(str::to_owned).collect();
            want.reverse();
            want.retain(|l| !l.is_empty());
            got.retain(|l| !l.is_empty());
            assert_eq!(got, want, "chunk {chunk}");
        }
        fs::write(&path, "").unwrap();
        let mut rev = RevLines::new(fs::File::open(&path).unwrap(), 4);
        assert_eq!(rev.next_line(), None, "an empty file");
    }

    // REQ-SEC-014
    #[test]
    fn nothing_the_model_or_an_event_chooses_can_break_the_file() {
        let log = AuditLog::in_memory(AuditConfig::default());
        let hostile = json!({"a\"b\n{\"id\":1}": "line1\n{\"v\":1,\"id\":99}\r\n"});
        let a = Act {
            tool: "x\n{\"v\":1}",
            tier: None,
            args: &hostile,
            decision: decision::INVALID,
            held_id: None,
            outcome: Some(Err("boom\n{\"id\":5}")),
            note: None,
        };
        log.record(1, 1, 1, "event:ev\nil", None, &a).unwrap();
        let page = log.query(&Query::default());
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].tier, "unknown");
        assert!(!page.records[0].tool.contains('\n'));
        assert_eq!(log.health().corrupt_lines, 0);
    }

    // REQ-SEC-016: the caps must hold when the log cannot be written (a full or read-only disk).
    #[test]
    fn an_action_counts_toward_the_caps_even_when_its_record_cannot_be_written() {
        let dir = TempDir::new("audit-unwritable");
        let log = AuditLog::open(&dir.0, AuditConfig::default()).unwrap();
        let act = |tool: &'static str| Act {
            tool,
            tier: Some(Tier::Costly),
            args: &Value::Null,
            decision: decision::RAN,
            held_id: None,
            outcome: Some(Ok("ok")),
            note: None,
        };
        assert!(
            log.record(100, 1, 1, "voice", None, &act("first"))
                .is_some()
        );
        assert_eq!(log.today(1), (1, 1));
        // Put a directory where the log file is, so the next append fails.
        for entry in std::fs::read_dir(&dir.0).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
            }
        }
        assert!(
            log.record(101, 1, 1, "voice", None, &act("second"))
                .is_none()
        );
        assert_eq!(log.today(1), (2, 2), "the action ran, so it counts");
        assert!(log.health().write_errors >= 1);
    }
}
