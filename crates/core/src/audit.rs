//! The audit log of everything the assistant does (REQ-SEC-014), and the per-day action caps and dry-run
//! switch that sit beside it (REQ-SEC-015, REQ-SEC-016).
//!
//! One JSON object per line in `audit.jsonl` in the data directory (0600). The file is append-only and
//! size-capped: when it would pass the cap it is renamed to `audit.1.jsonl` (replacing the previous one) and a
//! new file starts, so the disk holds at most twice the cap. Start-up tolerates a crash: a half-written last
//! line or any damaged line is skipped, never an error.
//!
//! The day's counts of actions are also kept in their own small file, `audit-counts.json`, because the log
//! forgets: it keeps only the current file and one rotated file, so on a busy day a restart that recounted from
//! the log alone would undercount and loosen the caps. At start the larger of the two counts is used.
//!
//! The log is written from the session loop with text that may come from outside (a tool result, a model's
//! arguments). So everything is made safe on the way in: secrets and tokens are replaced, each value is
//! clipped, control characters are removed, and nothing the user said or any audio is ever written.

use crate::app::persist::Writer;
use crate::sched::Ts;
use crate::session::Tier;
use crate::store::JsonStore;
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

impl Record {
    /// The record of one act, with every name and text clipped, redacted and bounded.
    fn of(
        id: u64,
        ts: Ts,
        day: i64,
        turn: u64,
        trigger: &str,
        cost: Option<f64>,
        a: &Act<'_>,
    ) -> Self {
        let (status, result) = match a.outcome {
            None => (None, None),
            Some(Ok(text)) => (Some("ok"), Some(excerpt(text))),
            Some(Err(text)) => (Some("error"), Some(excerpt(text))),
        };
        let detail = match a.outcome {
            Some(Ok(text)) if text.chars().count() > LONG_RESULT_CHARS => Some(redact_detail(text)),
            _ => None,
        };
        Self {
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
        }
    }
}

/// The record as one line of JSON that fits a line of the log. A long detail is shortened, and dropped if it
/// still does not fit, rather than the record being lost. `None` when it cannot be serialised or cannot be
/// made to fit.
fn line_of(rec: &mut Record) -> Option<String> {
    let mut line = serde_json::to_string(rec).ok()?;
    if line.len() > MAX_LINE_BYTES && rec.detail.is_some() {
        // Multi-byte text can make a long detail too big for one line: keep less rather than lose the record.
        rec.detail = rec.detail.take().map(|d| clip(&d, 500));
        line = serde_json::to_string(rec).ok()?;
        if line.len() > MAX_LINE_BYTES {
            rec.detail = None;
            line = serde_json::to_string(rec).ok()?;
        }
    }
    if line.len() > MAX_LINE_BYTES {
        // Cannot happen with the clips above; refuse rather than write something unbounded.
        return None;
    }
    Some(line)
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

/// What `audit-counts.json` holds: the counts for one local day.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
struct SavedCounts {
    day: i64,
    actions: u32,
    costly: u32,
}

/// The counts file's schema version.
const COUNTS_SCHEMA: u32 = 1;

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
    /// Where the day's counts are kept besides the log (`None` for the in-memory log).
    counts_store: Option<JsonStore>,
    /// The background writer, when the core has one; without it the counts file is written in place.
    writer: Option<Arc<Writer>>,
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
                counts_store: None,
                writer: None,
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
                counts_store: Some(JsonStore::new(dir.join("audit-counts.json"), COUNTS_SCHEMA)),
                writer: None,
                corrupt_lines: corrupt,
                write_errors: 0,
            }),
        })
    }

    /// Hand the counts file to the background writer, so recording an action does not wait for a second write
    /// to flash. Without one the file is written in place.
    pub fn set_writer(&self, writer: Option<Arc<Writer>>) {
        lock(&self.state).writer = writer;
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
        // The day the counts are for, which is not the day asked for while the clock is unset (see load_counts).
        let day = s.load_counts(day);
        let mut line = line_of(&mut Record::of(id, ts, day, turn, trigger, cost, a))?;
        line.push('\n');
        // The daily caps count what ran, whether or not the record could be written: a full or read-only disk
        // must not turn the caps off (REQ-SEC-016).
        let counted = a.decision == decision::RAN || a.decision == decision::APPROVED;
        if counted && a.tier.is_some_and(|t| t != Tier::ReadOnly) {
            s.counts.actions = s.counts.actions.saturating_add(1);
            if a.tier >= Some(Tier::Costly) {
                s.counts.costly = s.counts.costly.saturating_add(1);
            }
            s.save_counts();
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
                let newline_first = *needs_newline;
                let written = (|| {
                    if *needs_newline {
                        f.write_all(b"\n")?;
                        *needs_newline = false;
                    }
                    f.write_all(line.as_bytes())
                })();
                if written.is_err() {
                    // A write that failed part way (a full disk) may have left half a line: the next record
                    // starts on a new line, and the size is what the file holds, not what was meant.
                    *needs_newline = true;
                    *size = fs::metadata(&*path).map_or(*size, |m| m.len());
                    return written;
                }
                *size += len + u64::from(newline_first);
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

    /// Make the day's counters match the log (it is the record of what ran), the first time a day is seen, and
    /// return the day they are for. After that [`AuditLog::record`] keeps them up to date, so this does not run
    /// again for the same day.
    ///
    /// The day asked for is not always the day used. 0 means the wall clock is not set, and a day below the one
    /// already held means the clock stepped back: both keep counting on the day held (or, on the first call, on
    /// the newest day the log or the counts file holds), so a device with no battery clock cannot reset the
    /// daily caps by booting in 1970 (REQ-SEC-015).
    fn load_counts(&mut self, day: i64) -> i64 {
        if self.counts.known {
            if day <= self.counts.day {
                return self.counts.day;
            }
            if self.counts.day == 0 {
                // Counted before any real day was known: keep it, on the first real day.
                self.counts.day = day;
                // The file still says day 0; without this a restart before the next action would lose them.
                self.save_counts();
                return day;
            }
        }
        let saved = self
            .counts_store
            .as_ref()
            .and_then(|st| st.load::<SavedCounts>().ok());
        let mut day = day;
        // The oldest real day anywhere in the log or the counts file. A record written before the clock was
        // valid carries day 0, and a day is only ever recorded as 0 while no real day was known, so every such
        // record is older than every real one: it belongs on the first real day, which is this one.
        let mut oldest = saved.map_or(0, |s| s.day).max(0);
        let mut newest = oldest;
        self.source().each_line(|line| {
            if let Some(r) = parse_line(line).filter(|r| r.day > 0) {
                newest = newest.max(r.day);
                oldest = if oldest == 0 {
                    r.day
                } else {
                    oldest.min(r.day)
                };
            }
        });
        if !self.counts.known {
            day = day.max(newest);
        }
        let first_real = if oldest == 0 { day } else { oldest };
        let mut c = Counts {
            day,
            known: true,
            actions: 0,
            costly: 0,
        };
        self.source().each_line(|line| {
            let Some(r) = parse_line(line) else { return };
            if (if r.day == 0 { first_real } else { r.day }) != day {
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
        // The log may have rotated away some of the day; the counts file has not forgotten them. Take the larger.
        // A missing or unreadable file changes nothing: the recount stands.
        let saved_day = |s: &SavedCounts| if s.day == 0 { first_real } else { s.day };
        if let Some(saved) = saved.filter(|s| saved_day(s) == day) {
            c.actions = c.actions.max(saved.actions);
            c.costly = c.costly.max(saved.costly);
        }
        self.counts = c;
        day
    }

    /// Write the day's counts to their own file. A failure is reported, never fatal: the counts in memory still
    /// hold, so a full or read-only disk does not turn the caps off (REQ-SEC-016).
    fn save_counts(&self) {
        let Some(store) = &self.counts_store else {
            return;
        };
        let saved = SavedCounts {
            day: self.counts.day,
            actions: self.counts.actions,
            costly: self.counts.costly,
        };
        if let Some(w) = &self.writer {
            w.submit_with(store, || store.encode(&saved));
        } else if let Err(e) = store.save(&saved) {
            crate::diag::error(&format!("could not save the action counts: {e}"));
        }
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

/// Words that name a secret when they are a whole word of a key (`api_key`, `apiKey`, `X-Auth-Token`).
const SECRET_WORDS: [&str; 21] = [
    "pin",
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "passcode",
    "key",
    "auth",
    "oauth",
    "authorization",
    "authentication",
    "bearer",
    "jwt",
    "cookie",
    "credential",
    "signature",
    "session",
    "otp",
    "apikey",
];

/// Endings that name a secret when a key is written as one word (`accesstoken`, `clientsecret`, `sessionid`).
const SECRET_ENDINGS: [&str; 12] = [
    "token",
    "secret",
    "password",
    "passwd",
    "passphrase",
    "apikey",
    "accesskey",
    "secretkey",
    "privatekey",
    "authkey",
    "passkey",
    "sessionid",
];

/// The words of a key, in lower case: split at anything that is not a letter or digit, and where a capital
/// follows a small letter or digit (`apiKey`).
fn key_words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut after_small = false;
    for c in key.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            after_small = false;
            continue;
        }
        if c.is_uppercase() && after_small && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        after_small = c.is_lowercase() || c.is_numeric();
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Whether a key names something secret. It is decided on the key's whole words, so `keyword`, `author`,
/// `monkey` or `privately` are not taken for secrets: a value hidden for no reason is one the person cannot
/// hear before they say yes to an action (REQ-SEC-011), and one missing from the record of what ran.
fn sensitive_key(key: &str) -> bool {
    key_words(key).iter().any(|w| {
        // `tokens`, `keys`, `credentials`.
        let one = w.strip_suffix('s').filter(|b| b.len() >= 3).unwrap_or(w);
        SECRET_WORDS.contains(&one)
            || SECRET_WORDS.contains(&w.as_str())
            || SECRET_ENDINGS.iter().any(|e| one.ends_with(e))
    })
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
        ) || names_next_word_secret(bare);
        out.push(replaced);
    }
    out.join(" ")
}

/// Whether a word such as `api_key:` or `pin=` ends in a colon or equals sign after a secret key, so that the
/// next word is its value (`api_key: abc123`). A key that is also an everyday word on its own (`key:`,
/// `session:`) does not count, or ordinary sentences would be taken for secrets; with a second word
/// (`api_key`, `session_id`) or a clearer one (`pin`, `passphrase`) it does.
fn names_next_word_secret(bare: &str) -> bool {
    let Some(key) = bare
        .strip_suffix(':')
        .or_else(|| bare.strip_suffix('='))
        .map(|k| k.trim_matches(|c: char| !c.is_alphanumeric()))
    else {
        return false;
    };
    if key.is_empty() || key.contains(char::is_whitespace) {
        return false;
    }
    let words = key_words(key);
    if words.len() == 1
        && matches!(
            words[0].as_str(),
            "key" | "session" | "signature" | "cookie" | "auth"
        )
    {
        return false;
    }
    sensitive_key(key)
}

/// Whether a path segment of a URL is a secret (a webhook or an invite token): long, with no spaces, and not
/// an all-lower-case slug.
fn secret_path_segment(seg: &str) -> bool {
    if looks_like_token(seg) {
        return true;
    }
    seg.len() >= 20
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        && (seg.chars().any(|c| c.is_ascii_digit()) && seg.chars().any(|c| c.is_ascii_alphabetic())
            || seg.chars().any(|c| c.is_ascii_uppercase())
                && seg.chars().any(|c| c.is_ascii_lowercase())
            || seg.chars().all(|c| c.is_ascii_uppercase()))
}

/// Redact the path segments of a URL that are secrets, keeping the host and the rest.
fn redact_path(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_owned();
    };
    let start = scheme_end + 3;
    let end = url[start..]
        .find(['?', '#'])
        .map_or(url.len(), |i| start + i);
    let Some(slash) = url[start..end].find('/') else {
        return url.to_owned();
    };
    let path_start = start + slash;
    let path: Vec<String> = url[path_start..end]
        .split('/')
        .map(|seg| {
            if secret_path_segment(seg) {
                REDACTED.to_owned()
            } else {
                seg.to_owned()
            }
        })
        .collect();
    format!("{}{}{}", &url[..path_start], path.join("/"), &url[end..])
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
            return redact_path(&redact_query(&format!("{head}{REDACTED}@{tail}")));
        }
        return redact_path(&redact_query(word));
    }
    if bare.contains('?') || bare.contains('&') || bare.contains('#') {
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

/// Redact `name=value` pairs in a query string and in a fragment (`#access_token=abc`).
fn redact_query(s: &str) -> String {
    let Some(q) = s.find(['?', '#']) else {
        return s.to_owned();
    };
    let (head, rest) = s.split_at(q + 1);
    // `rest` is the query, then perhaps a `#` and the fragment; or, when the first mark was `#`, the fragment.
    let (first, fragment) = match rest.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    let mut out = format!("{head}{}", redact_pairs(first));
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(&redact_pairs(f));
    }
    out
}

/// `a=b&c=d` with the values of secret names, and token-like values, replaced.
fn redact_pairs(pairs: &str) -> String {
    let parts: Vec<String> = pairs
        .split('&')
        .map(|p| match p.split_once('=') {
            Some((k, v)) if !v.is_empty() && (sensitive_key(k) || looks_like_token(v)) => {
                format!("{k}={REDACTED}")
            }
            _ => p.to_owned(),
        })
        .collect();
    parts.join("&")
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
                // Two keys can clean to one name; both are kept (`a_b`, `a_b_2`) so the log cannot show one
                // value where the tool ran with the other.
                let name = clean_name(k);
                let mut unique = name.clone();
                let mut n = 2;
                while out.contains_key(&unique) {
                    unique = format!("{name}_{n}");
                    n += 1;
                }
                out.insert(unique, value);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Whether a name (an argument key) names something secret. One implementation for the log and for speech.
#[must_use]
pub fn is_secret_name(key: &str) -> bool {
    sensitive_key(key)
}

/// The secrets in `text` replaced, with nothing clipped, for the read-back of a held action (REQ-SEC-011): the
/// same word rules as [`redact_text`] but the whole text is kept, because the user must hear all of what will
/// be sent. Control characters become spaces.
#[must_use]
pub fn redact_unclipped(text: &str) -> String {
    redact_words(&printable(text))
}

/// The marker [`redact_unclipped`] leaves where it removed a secret.
pub const REDACTED_MARK: &str = REDACTED;

/// Strings longer than this are shortened to "a long value" in [`spoken_args`].
pub const MAX_SPOKEN_VALUE_CHARS: usize = 60;

/// Arguments in a short, redacted form for the recent-actions list the model sees: the same redaction as the
/// audit log (`redact_value`), and any text value longer than [`MAX_SPOKEN_VALUE_CHARS`] replaced by "a long
/// value". Not the read-back of a held action, which says everything (`session::describe_generic`).
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
mod tests;
