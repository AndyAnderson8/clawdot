//! Scenario-based evaluation of the agent loop (REQ-AST-016..018; `docs/architecture/evals.md`).
//!
//! * [`scenario`]: the JSON file format and loading.
//! * [`run`]: runs a scenario through the product's real session loop and checks the result.
//! * [`model`]: the scripted fake model (replay) and the real endpoint (live).
//! * [`live`]: live mode: three runs per scenario under a spend cap, with pass rates.
//!
//! This is a development tool. It is not linked into the `clawdot` binary, so it costs the device nothing.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod host;
pub mod live;
pub mod matcher;
pub mod model;
pub mod run;
pub mod scenario;

use clawdot_core::usage::{UsageLimits, UsageTracker};
use run::{RunReport, run_scenario};
use scenario::Loaded;
use std::path::PathBuf;

/// Where the shipped scenarios live: `evals/scenarios` at the repository root.
#[must_use]
pub fn default_scenarios_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("evals")
        .join("scenarios")
}

/// Run one scenario in replay mode: scripted model, no network, no spend.
pub async fn replay_one(l: &Loaded) -> RunReport {
    let model = model::Scripted::new();
    let mut usage = UsageTracker::new(UsageLimits::default());
    run_scenario(&l.scenario, &model, &mut usage).await
}

/// A fingerprint of the scenario files, so a recorded result names the set it was measured on.
#[must_use]
pub fn set_fingerprint(scenarios: &[Loaded]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for l in scenarios {
        let bytes = std::fs::read(&l.path).unwrap_or_default();
        for b in l.scenario.id.bytes().chain(bytes) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{:08x}", h & 0xffff_ffff)
}

/// A readable line per failed check.
#[must_use]
pub fn describe_failures(r: &RunReport) -> String {
    let mut out = String::new();
    for c in r.failures() {
        let at = c.turn.map_or_else(String::new, |t| format!("turn {t}: "));
        let safety = if c.safety { " [SAFETY]" } else { "" };
        out.push_str(&format!("      {at}{}{safety}: {}\n", c.name, c.detail));
    }
    if r.stopped_by_limit {
        out.push_str("      stopped by a usage limit\n");
    }
    out
}
