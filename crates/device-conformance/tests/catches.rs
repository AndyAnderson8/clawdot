//! The suite must fail boards that break the contract, or a green run means nothing. Each board here breaks one
//! requirement on purpose; the matching case must fail and the others must be unaffected.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_device::board::Parts;
use clawdot_device::board_file::{BoardFile, MuteKind};
use clawdot_device::caps::{Build, Declared};
use clawdot_device_conformance::{
    BoardUnderTest, FileBoard, Fixture, Outcome, Profile, run_all, run_case,
};
use clawdot_satellite::audio::{AudioError, AudioOut};

const DESKTOP: &str = include_str!("../../../boards/desktop.json");

/// A speaker that ignores `stop`.
struct DeafOut(Box<dyn AudioOut>);

impl AudioOut for DeafOut {
    fn begin(&mut self) -> Result<(), AudioError> {
        self.0.begin()
    }
    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        self.0.play(pcm)
    }
    fn finish(&mut self) {
        self.0.finish();
    }
    fn poll_done(&mut self) -> Result<bool, AudioError> {
        self.0.poll_done()
    }
    fn stop(&mut self) {}
}

#[derive(Clone, Copy)]
enum Fault {
    /// Declares a hardware mute the board file does not have.
    LiesAboutMute,
    /// The speaker keeps playing after a stop.
    DeafStop,
}

struct Faulty(Fault);

impl BoardUnderTest for Faulty {
    fn name(&self) -> String {
        "faulty".into()
    }

    fn board_json(&self) -> String {
        DESKTOP.into()
    }

    fn build(&self, profile: Profile, fx: &Fixture) -> Result<Parts, String> {
        let mut parts = FileBoard::new(DESKTOP).build(profile, fx)?;
        match self.0 {
            Fault::LiesAboutMute => {
                let mut f = BoardFile::parse(DESKTOP).map_err(|e| e.to_string())?;
                f.mute.kind = MuteKind::Hardware;
                parts.declared = Declared::from_board(&f, Build::CURRENT);
            }
            Fault::DeafStop => parts.output = Box::new(DeafOut(parts.output)),
        }
        Ok(parts)
    }
}

fn failed(o: &Outcome) -> bool {
    matches!(o, Outcome::Fail(_))
}

#[test]
fn the_good_board_passes_everything_the_faulty_ones_fail() {
    let good = run_all(&FileBoard::new(DESKTOP));
    assert!(good.passed(), "{:?}", good.failures());
}

#[test]
fn req_dev_015_a_board_that_claims_a_hardware_mute_it_lacks_fails() {
    let b = Faulty(Fault::LiesAboutMute);
    assert!(failed(&run_case(
        &b,
        "req_dev_002_hello_declares_exactly_the_present_capabilities"
    )));
    assert!(failed(&run_case(&b, "req_dev_015_mute_is_honest")));
    // The faults do not leak into unrelated requirements.
    assert!(!failed(&run_case(
        &b,
        "req_dev_005_stop_without_the_wake_word"
    )));
    assert!(!failed(&run_case(&b, "req_dev_019_forbidden_paths")));
}

#[test]
fn req_dev_005_a_speaker_that_ignores_stop_fails() {
    let b = Faulty(Fault::DeafStop);
    assert!(failed(&run_case(
        &b,
        "req_dev_005_stop_without_the_wake_word"
    )));
    assert!(!failed(&run_case(&b, "req_dev_015_mute_is_honest")));
}

#[test]
fn req_dev_008_a_board_file_with_an_unknown_key_fails_the_suite() {
    // The loader refuses a stray key, so the suite reports the board as non-conforming.
    let bad = DESKTOP.replacen("\"schema\": 1", "\"schema\": 1, \"colour\": \"red\"", 1);
    let b = FileBoard::new(bad);
    let r = run_case(&b, "req_dev_008_board_file_validation");
    assert!(failed(&r), "{r:?}");
}

#[test]
fn an_unknown_case_is_a_failure_not_a_pass() {
    assert!(failed(&run_case(
        &FileBoard::new(DESKTOP),
        "req_dev_999_nothing"
    )));
}
