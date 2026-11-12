//! ClawDot agent bridge: pulls voice replies from the device and resumes the agent.
//!
//! The device never calls out to agent hosts (ADR-0006). This program runs where the agent runs, opens
//! a stream to the device, and for each reply either runs the harness's resume command or just
//! acknowledges it. Delivery is at-least-once, so replies are de-duplicated on their id.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod client;
pub mod harness;
pub mod sse;
pub mod state;

use clawdot_protocol::bridge::Reply;
use client::{Flow, Url};
use harness::{CommandSpec, Harness, Skip};
use std::io;
use std::time::Duration;

/// Everything the bridge needs to run.
#[derive(Debug)]
pub struct Config {
    /// The device.
    pub url: Url,
    /// The endpoint name to listen on.
    pub endpoint: String,
    /// The endpoint token.
    pub token: String,
    /// How long to wait for any bytes (heartbeat is every 15 s).
    pub read_timeout: Duration,
    /// How a reply resumes the agent.
    pub harness: Box<dyn Harness>,
}

/// Runs a command. Swapped for a fake in tests and for a printer with `--dry-run`.
pub trait Runner {
    /// Run to completion and return the exit code.
    ///
    /// # Errors
    /// The process could not be started.
    fn run(&mut self, spec: &CommandSpec) -> io::Result<i32>;
}

/// Runs real processes, inheriting stdio so the agent's output is visible.
#[derive(Debug, Default)]
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<i32> {
        let status = std::process::Command::new(&spec.program)
            .args(&spec.args)
            .status()?;
        Ok(status.code().unwrap_or(-1))
    }
}

/// Prints what would run, without running it.
#[derive(Debug, Default)]
pub struct DryRunner;

impl Runner for DryRunner {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<i32> {
        println!("clawdot-bridge: [dry run] {} {:?}", spec.program, spec.args);
        Ok(0)
    }
}

/// What happened to one reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handling {
    /// The command ran and exited with this code.
    Ran(i32),
    /// No command applied (acknowledgement, bad session, no text).
    Skipped(Skip),
    /// Already handled earlier; only re-acknowledged.
    Duplicate,
    /// The command could not be started; left unacknowledged so it is retried.
    CouldNotStart,
}

/// Handle one reply: de-duplicate, run, record, acknowledge.
///
/// A command that cannot even be started is *not* recorded or acknowledged, so the reply comes back on
/// the next connection; a command that ran (whatever its exit code) is recorded and acknowledged, so an
/// instruction is never executed twice.
pub fn handle_reply(
    harness: &dyn Harness,
    handled: &mut state::Handled,
    runner: &mut dyn Runner,
    reply: &Reply,
    ack: &mut dyn FnMut(u64) -> io::Result<u16>,
) -> Handling {
    let confirm = |ack: &mut dyn FnMut(u64) -> io::Result<u16>| {
        if let Err(e) = ack(reply.id) {
            eprintln!(
                "clawdot-bridge: could not acknowledge reply {}: {e}",
                reply.id
            );
        }
    };
    let key = state::Key::of(reply);
    if handled.contains(key) {
        confirm(ack);
        return Handling::Duplicate;
    }
    match harness.plan(reply) {
        Err(skip) => {
            if let Err(e) = handled.insert(key) {
                eprintln!("clawdot-bridge: could not save state: {e}");
            }
            confirm(ack);
            Handling::Skipped(skip)
        }
        Ok(spec) => {
            // Record before running: if the bridge dies mid-run, the instruction is not repeated.
            if let Err(e) = handled.insert(key) {
                eprintln!("clawdot-bridge: could not save state: {e}");
            }
            match runner.run(&spec) {
                Ok(code) => {
                    if code != 0 {
                        eprintln!("clawdot-bridge: {} exited with code {code}", spec.program);
                    }
                    confirm(ack);
                    Handling::Ran(code)
                }
                Err(e) => {
                    eprintln!("clawdot-bridge: could not start {}: {e}", spec.program);
                    // Nothing ran, so forget the id: the unacknowledged reply comes back on reconnect.
                    if let Err(e) = handled.remove(key) {
                        eprintln!("clawdot-bridge: could not save state: {e}");
                    }
                    Handling::CouldNotStart
                }
            }
        }
    }
}

/// Connect once and handle replies until the stream ends, `max` replies were processed, or an error
/// occurs. Returns how many replies were processed.
///
/// # Errors
/// Connection and protocol errors; a rejected token is `PermissionDenied`.
pub fn stream_once(
    cfg: &Config,
    handled: &mut state::Handled,
    runner: &mut dyn Runner,
    max: Option<usize>,
) -> io::Result<usize> {
    let mut count = 0_usize;
    let url = cfg.url.clone();
    let (endpoint, token) = (cfg.endpoint.clone(), cfg.token.clone());
    client::stream_replies(
        &cfg.url,
        &cfg.endpoint,
        &cfg.token,
        cfg.read_timeout,
        &mut |reply| {
            let mut ack = |id: u64| client::ack(&url, &endpoint, &token, id);
            let result = handle_reply(&*cfg.harness, handled, runner, &reply, &mut ack);
            println!("clawdot-bridge: reply {} -> {result:?}", reply.id);
            count += 1;
            if max.is_some_and(|m| count >= m) {
                Flow::Stop
            } else {
                Flow::Continue
            }
        },
    )?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawdot_protocol::bridge::ReplyKind;
    use harness::ClaudeCode;

    struct FakeRunner {
        ran: Vec<CommandSpec>,
        result: io::Result<i32>,
    }
    impl FakeRunner {
        fn ok(code: i32) -> Self {
            Self {
                ran: Vec::new(),
                result: Ok(code),
            }
        }
        fn failing() -> Self {
            Self {
                ran: Vec::new(),
                result: Err(io::Error::new(io::ErrorKind::NotFound, "no claude")),
            }
        }
    }
    impl Runner for FakeRunner {
        fn run(&mut self, spec: &CommandSpec) -> io::Result<i32> {
            self.ran.push(spec.clone());
            match &self.result {
                Ok(c) => Ok(*c),
                Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
            }
        }
    }

    fn reply(id: u64, kind: ReplyKind, session: Option<&str>, text: &str) -> Reply {
        Reply {
            id,
            endpoint: "e".into(),
            event_id: 1,
            session_ref: session.map(str::to_owned),
            kind,
            text: text.into(),
            created_at: 0,
        }
    }

    fn handle(
        r: &Reply,
        handled: &mut state::Handled,
        runner: &mut FakeRunner,
    ) -> (Handling, Vec<u64>) {
        let mut acked = Vec::new();
        let mut ack = |id: u64| {
            acked.push(id);
            Ok(204)
        };
        let out = handle_reply(&ClaudeCode::default(), handled, runner, r, &mut ack);
        (out, acked)
    }

    #[test]
    fn an_instruction_runs_once_and_is_acknowledged() {
        let (mut h, mut run) = (state::Handled::in_memory(), FakeRunner::ok(0));
        let (out, acked) = handle(
            &reply(1, ReplyKind::Instruct, Some("s1"), "add tests"),
            &mut h,
            &mut run,
        );
        assert_eq!((out, acked), (Handling::Ran(0), vec![1]));
        assert_eq!(run.ran.len(), 1);
    }

    #[test]
    fn redelivery_never_runs_an_instruction_twice() {
        let (mut h, mut run) = (state::Handled::in_memory(), FakeRunner::ok(0));
        let r = reply(1, ReplyKind::Instruct, Some("s1"), "add tests");
        handle(&r, &mut h, &mut run);
        let (out, acked) = handle(&r, &mut h, &mut run);
        assert_eq!((out, acked), (Handling::Duplicate, vec![1]));
        assert_eq!(run.ran.len(), 1, "still just one run");
    }

    #[test]
    fn a_reply_from_a_speaker_that_was_reset_runs_although_its_number_was_seen_before() {
        let (mut h, mut run) = (state::Handled::in_memory(), FakeRunner::ok(0));
        handle(
            &reply(1, ReplyKind::Instruct, Some("s1"), "add tests"),
            &mut h,
            &mut run,
        );
        // The speaker's data was reset, so it numbers its replies from one again.
        let mut fresh = reply(1, ReplyKind::Instruct, Some("s2"), "ship it");
        fresh.created_at = 500;
        let (out, acked) = handle(&fresh, &mut h, &mut run);
        assert_eq!((out, acked), (Handling::Ran(0), vec![1]));
        assert_eq!(run.ran.len(), 2);
    }

    #[test]
    fn an_acknowledgement_runs_nothing_but_is_confirmed() {
        let (mut h, mut run) = (state::Handled::in_memory(), FakeRunner::ok(0));
        let (out, acked) = handle(&reply(2, ReplyKind::Ack, Some("s1"), ""), &mut h, &mut run);
        assert_eq!((out, acked), (Handling::Skipped(Skip::Ack), vec![2]));
        assert!(run.ran.is_empty());
    }

    #[test]
    fn a_command_that_cannot_start_is_retried_later() {
        let (mut h, mut run) = (state::Handled::in_memory(), FakeRunner::failing());
        let r = reply(3, ReplyKind::Instruct, Some("s1"), "go");
        let (out, acked) = handle(&r, &mut h, &mut run);
        assert_eq!(
            (out, acked.is_empty()),
            (Handling::CouldNotStart, true),
            "not acknowledged"
        );
        assert!(
            !h.contains(state::Key::of(&r)),
            "a start failure must stay retryable"
        );
        // When the problem is fixed, the redelivered reply runs.
        let mut fixed = FakeRunner::ok(0);
        let (out, acked) = handle(&r, &mut h, &mut fixed);
        assert_eq!(
            (out, acked, fixed.ran.len()),
            (Handling::Ran(0), vec![3], 1)
        );
    }

    #[test]
    fn a_nonzero_exit_still_counts_as_handled() {
        let (mut h, mut run) = (state::Handled::in_memory(), FakeRunner::ok(1));
        let (out, acked) = handle(
            &reply(4, ReplyKind::Instruct, Some("s1"), "go"),
            &mut h,
            &mut run,
        );
        assert_eq!((out, acked), (Handling::Ran(1), vec![4]));
    }

    #[test]
    fn unusable_replies_are_skipped_and_confirmed_so_they_do_not_loop() {
        let (mut h, mut run) = (state::Handled::in_memory(), FakeRunner::ok(0));
        let (out, acked) = handle(
            &reply(5, ReplyKind::Instruct, Some("--evil"), "go"),
            &mut h,
            &mut run,
        );
        assert_eq!((out, acked), (Handling::Skipped(Skip::BadSession), vec![5]));
        assert!(run.ran.is_empty());
    }
}
