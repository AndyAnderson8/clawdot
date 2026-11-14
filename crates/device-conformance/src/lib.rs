//! The device conformance suite (spec 0006 section 9, REQ-DEV-011).
//!
//! It runs the device layer contract against **any board** with a fake core. A board is anything that implements
//! [`BoardUnderTest`]: it supplies its board file and builds [`Parts`] from a [`Fixture`] (recorded speaker,
//! scripted microphone, captured LED output). The generic [`FileBoard`] does this for any board file, so adding a
//! device is adding a file; a board with real drivers implements the trait itself.
//!
//! Each case is named for the requirement it covers (`req_dev_005_...`) and ends in a [`Outcome`]: pass, skip with
//! a stated reason (the requirement cannot be shown off the hardware, or the feature is not built), or fail.
//! [`run_all`] returns a [`Report`] whose Markdown is what a board records to count as supported.
//!
//! To add the suite for a board, write a test file that calls [`conformance_tests!`] with your board.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod cases;
mod fake;

pub use cases::{Case, Pri, cases};
pub use fake::{CoreConn, Rec, Session, SharedBuf, run_session};

use clawdot_device::board::{Board, ConfiguredBoard, Parts};
use clawdot_device::board_file::{BoardFile, ButtonSource, MuteKind};
use clawdot_satellite::audio::{AudioIn, WavIn};
use std::io::Cursor;

/// Which variant of the board a case builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The board as its file describes it.
    Full,
    /// The same board with no LEDs, no buttons and no mute control: exercises the documented equivalents.
    Stripped,
}

/// What a case gives a board to build from.
#[derive(Debug)]
pub struct Fixture {
    /// Microphone samples the board's input will deliver, then silence.
    pub mic: Vec<i16>,
    /// Terminal keys typed, one line each, for boards whose buttons are keys.
    pub keys: String,
    /// The recorded speaker.
    pub rec: Rec,
    /// Console LED output.
    pub led_out: fake::SharedBuf,
}

impl Fixture {
    /// A fixture with a silent microphone and no keys.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mic: Vec::new(),
            keys: String::new(),
            rec: Rec::default(),
            led_out: fake::SharedBuf::default(),
        }
    }

    /// Microphone audio as a source.
    #[must_use]
    pub fn mic_source(&self) -> Box<dyn AudioIn> {
        Box::new(WavIn::new(self.mic.clone(), 0))
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

/// A board that can be put through the suite.
pub trait BoardUnderTest {
    /// Board id for the report.
    fn name(&self) -> String;
    /// The board file as JSON text (the suite mutates copies of it to check validation).
    fn board_json(&self) -> String;
    /// Build the board's parts for a case.
    ///
    /// # Errors
    /// The board cannot be built; the text says why.
    fn build(&self, profile: Profile, fx: &Fixture) -> Result<Parts, String>;
    /// True when the suite runs on real hardware with real audio: cases that need to observe the speaker or feed
    /// the microphone are then skipped with a reason.
    fn on_hardware(&self) -> bool {
        false
    }
}

/// The generic board: the daemon's own [`ConfiguredBoard`] over any board file, with audio replaced by the
/// fixture. This is what runs in CI for the desktop board, and what a new board's file is checked with before
/// it is run on the unit.
#[derive(Debug, Clone)]
pub struct FileBoard {
    json: String,
}

impl FileBoard {
    /// A board from board-file JSON.
    #[must_use]
    pub fn new(json: impl Into<String>) -> Self {
        Self { json: json.into() }
    }
}

fn stripped(file: &BoardFile) -> BoardFile {
    let mut f = file.clone();
    f.leds = None;
    f.buttons.clear();
    f.actions.clear();
    f.mute.kind = MuteKind::None;
    f
}

impl BoardUnderTest for FileBoard {
    fn name(&self) -> String {
        BoardFile::parse(&self.json).map_or_else(|_| "invalid".into(), |f| f.board)
    }

    fn board_json(&self) -> String {
        self.json.clone()
    }

    fn build(&self, profile: Profile, fx: &Fixture) -> Result<Parts, String> {
        let file = BoardFile::parse(&self.json).map_err(|e| e.to_string())?;
        let file = if profile == Profile::Stripped {
            stripped(&file)
        } else {
            file
        };
        let has_keys = file
            .buttons
            .iter()
            .any(|b| matches!(b.source, ButtonSource::Key(_)));
        let mut board = ConfiguredBoard::new(file)
            .with_audio(fx.mic_source(), Box::new(fx.rec.clone()))
            .with_led_output(Box::new(fx.led_out.clone()));
        if has_keys {
            board = board.with_key_reader(Box::new(Cursor::new(fx.keys.clone().into_bytes())));
        }
        Box::new(board).assemble()
    }
}

/// The result of one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The requirement holds.
    Pass,
    /// Not run, with the reason (needs hardware, or the feature is not built, or the board lacks the feature
    /// and the requirement allows that).
    Skip(String),
    /// The requirement does not hold.
    Fail(String),
}

/// All outcomes for a board.
#[derive(Debug, Clone)]
pub struct Report {
    /// Board id.
    pub board: String,
    /// One row per case: id, requirement, priority, outcome.
    pub rows: Vec<(&'static str, &'static str, Pri, Outcome)>,
}

impl Report {
    /// Whether every case passed or was skipped with a reason, and none failed. A skip that gives no reason counts
    /// as a failure: a skipped case must say why it was not run.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failures().is_empty()
    }

    /// Cases that failed, including skips that record no reason.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|(id, _, _, o)| match o {
                Outcome::Fail(m) => Some(format!("{id}: {m}")),
                Outcome::Skip(why) if why.trim().is_empty() => {
                    Some(format!("{id}: skipped without a reason"))
                }
                _ => None,
            })
            .collect()
    }

    /// Markdown to record a run: a table of cases and a summary line.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "## Conformance: {}\n\n| Case | Requirement | Priority | Result |\n|---|---|---|---|\n",
            self.board
        );
        let (mut p, mut s, mut f) = (0, 0, 0);
        for (id, req, pri, o) in &self.rows {
            let (res, note) = match o {
                Outcome::Pass => {
                    p += 1;
                    ("pass", String::new())
                }
                Outcome::Skip(why) => {
                    s += 1;
                    ("skip", format!(": {why}"))
                }
                Outcome::Fail(why) => {
                    f += 1;
                    ("FAIL", format!(": {why}"))
                }
            };
            out.push_str(&format!("| {id} | {req} | {pri:?} | {res}{note} |\n"));
        }
        out.push_str(&format!("\n{p} passed, {s} skipped, {f} failed.\n"));
        out
    }
}

/// Run one case by id.
///
/// # Panics
/// Never; an unknown id is reported as a failure outcome.
#[must_use]
pub fn run_case(board: &dyn BoardUnderTest, id: &str) -> Outcome {
    match cases().into_iter().find(|c| c.id == id) {
        Some(c) => (c.run)(board),
        None => Outcome::Fail(format!("no case named {id}")),
    }
}

/// Run every case.
#[must_use]
pub fn run_all(board: &dyn BoardUnderTest) -> Report {
    Report {
        board: board.name(),
        rows: cases()
            .into_iter()
            .map(|c| (c.id, c.req, c.pri, (c.run)(board)))
            .collect(),
    }
}

/// Generate one `#[test]` per case for a board, plus a test that the list matches the suite and one that prints
/// the Markdown report. `$board` is an expression producing a [`BoardUnderTest`].
///
/// ```ignore
/// clawdot_device_conformance::conformance_tests!(clawdot_device_conformance::FileBoard::new(
///     include_str!("../../../boards/desktop.json")
/// ));
/// ```
#[macro_export]
macro_rules! conformance_tests {
    ($board:expr) => {
        $crate::conformance_tests!(@each $board;
            req_dev_001_one_daemon_for_every_board,
            req_dev_002_hello_declares_exactly_the_present_capabilities,
            req_dev_003_audio_in_and_out,
            req_dev_004_led_states,
            req_dev_005_stop_without_the_wake_word,
            req_dev_006_quiet_hours_apply_locally,
            req_dev_007_updates_are_declared,
            req_dev_008_board_file_validation,
            req_dev_012_minimum_hardware,
            req_dev_013_absent_capabilities_have_equivalents,
            req_dev_014_tones_play_locally,
            req_dev_015_mute_is_honest,
            req_dev_016_ducking,
            req_dev_017_push_to_talk_and_wake_seam,
            req_dev_019_forbidden_paths,
            req_dev_020_local_functions_survive_a_lost_core,
            req_dev_021_default_build_links_no_c
        );

        #[test]
        fn conformance_case_list_matches_the_suite() {
            let listed: Vec<&str> = $crate::cases().iter().map(|c| c.id).collect();
            let known = $crate::MUST_CASES;
            for k in known {
                assert!(listed.contains(&k), "the suite has no case {k}");
            }
            assert_eq!(
                listed.iter().filter(|c| !$crate::REPORT_ONLY_CASES.contains(*c)).count(),
                known.len(),
                "a case is missing from conformance_tests!"
            );
        }

        #[test]
        fn conformance_report_has_no_failures() {
            let report = $crate::run_all(&$board);
            println!("{}", report.markdown());
            assert!(report.passed(), "{:?}", report.failures());
        }
    };
    (@each $board:expr; $($id:ident),*) => {
        $(
            #[test]
            fn $id() {
                match $crate::run_case(&$board, stringify!($id)) {
                    $crate::Outcome::Fail(why) => panic!("{why}"),
                    $crate::Outcome::Skip(why) => println!("skipped: {why}"),
                    $crate::Outcome::Pass => {}
                }
            }
        )*
    };
}

/// The ids of the cases [`conformance_tests!`] generates tests for.
pub const MUST_CASES: [&str; 17] = [
    "req_dev_001_one_daemon_for_every_board",
    "req_dev_002_hello_declares_exactly_the_present_capabilities",
    "req_dev_003_audio_in_and_out",
    "req_dev_004_led_states",
    "req_dev_005_stop_without_the_wake_word",
    "req_dev_006_quiet_hours_apply_locally",
    "req_dev_007_updates_are_declared",
    "req_dev_008_board_file_validation",
    "req_dev_012_minimum_hardware",
    "req_dev_013_absent_capabilities_have_equivalents",
    "req_dev_014_tones_play_locally",
    "req_dev_015_mute_is_honest",
    "req_dev_016_ducking",
    "req_dev_017_push_to_talk_and_wake_seam",
    "req_dev_019_forbidden_paths",
    "req_dev_020_local_functions_survive_a_lost_core",
    "req_dev_021_default_build_links_no_c",
];

/// Cases for requirements that are Should or May, not built yet, or only provable on the unit: reported in the run, never generated as tests.
pub const REPORT_ONLY_CASES: [&str; 6] = [
    "req_dev_009_follow_up_listening",
    "req_dev_010_wifi_setup",
    "req_dev_018_clock_trust",
    "req_dev_022_resource_profile",
    "req_dev_023_bluetooth_speaker_mode",
    "req_dev_024_spotify_connect",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn report(skip: &str) -> Report {
        Report {
            board: "x".into(),
            rows: vec![(
                "req_dev_009_x",
                "REQ-DEV-009",
                Pri::Must,
                Outcome::Skip(skip.into()),
            )],
        }
    }

    #[test]
    fn req_dev_005_a_skip_without_a_reason_does_not_pass() {
        let ok = report("needs a person on the unit");
        assert!(ok.passed() && ok.failures().is_empty());
        let bad = report("  ");
        assert!(!bad.passed(), "a skip must record why");
        assert_eq!(bad.failures().len(), 1);
        assert!(bad.failures()[0].contains("req_dev_009_x"));
    }
}
