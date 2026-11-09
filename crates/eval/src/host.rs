//! The tools a scenario offers: fixture tools that return what the scenario says, and the product's own
//! built-in timers and clock running for real against a fixed time. Real servers are never called.

use crate::scenario::{Scenario, ToolFixture};
use clawdot_core::assistant::Assistant;
use clawdot_core::cancel::CancelToken;
use clawdot_core::llm::ToolSpec;
use clawdot_core::sched::{Clock, FixedOffset, Scheduler, Ts};
use clawdot_core::session::{Risk, Tier, ToolHost};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// A clock the runner moves by hand.
#[derive(Debug)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    /// Starts at `t`.
    #[must_use]
    pub fn new(t: Ts) -> Self {
        Self(AtomicI64::new(t))
    }
    /// Move to `t`.
    pub fn set(&self, t: Ts) {
        self.0.store(t, Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Ts {
        self.0.load(Ordering::Relaxed)
    }
}

/// A call that actually ran (not one that was held or refused).
#[derive(Debug, Clone, PartialEq)]
pub struct Executed {
    /// Tool name.
    pub tool: String,
    /// The arguments it ran with.
    pub args: Value,
}

/// The tools of one scenario run.
#[derive(Debug)]
pub struct EvalHost {
    assistant: Assistant,
    builtin: Vec<String>,
    fixtures: Vec<ToolFixture>,
    calls: Vec<usize>,
    clock: Arc<ManualClock>,
    cancel: CancelToken,
    /// Every call that started, in order (a call that hangs or is cancelled started).
    pub executed: Vec<Executed>,
}

impl EvalHost {
    /// Tools for `s`, with the built-ins reading `clock`.
    #[must_use]
    pub fn new(s: &Scenario, clock: Arc<ManualClock>) -> Self {
        let assistant = Assistant::new(
            Arc::clone(&clock) as Arc<dyn Clock + Send + Sync>,
            Box::new(FixedOffset(0)),
            Scheduler::new(64),
            None,
            None,
        );
        Self {
            assistant,
            builtin: s.builtin_tools.clone(),
            calls: vec![0; s.tools.len()],
            fixtures: s.tools.clone(),
            clock,
            cancel: CancelToken::new(),
            executed: Vec::new(),
        }
    }

    /// The token of the turn about to run, for a tool that says the user stops it.
    pub fn set_cancel(&mut self, token: CancelToken) {
        self.cancel = token;
    }

    fn fixture(&self, name: &str) -> Option<(usize, &ToolFixture)> {
        self.fixtures
            .iter()
            .enumerate()
            .find(|(_, t)| t.name == name)
    }

    fn is_builtin(&self, name: &str) -> bool {
        self.builtin.iter().any(|b| b == name)
    }
}

impl ToolHost for EvalHost {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut v: Vec<ToolSpec> = self
            .assistant
            .tool_specs()
            .into_iter()
            .filter(|t| self.is_builtin(&t.name))
            .collect();
        v.extend(self.fixtures.iter().map(|t| ToolSpec {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        }));
        v
    }

    fn risk(&self, name: &str, args: &Value) -> Risk {
        if self.is_builtin(name) {
            return self.assistant.risk(name, args);
        }
        let Some((_, t)) = self.fixture(name) else {
            return Risk::fixed(Tier::Costly);
        };
        let tier = t
            .tier_by_arg
            .as_ref()
            .and_then(|b| {
                let v = args.get(&b.arg)?.as_str()?;
                b.values.get(v).copied()
            })
            .unwrap_or(t.tier);
        match t.lowered_from {
            Some(base) => Risk::set(tier, base),
            None => Risk::fixed(tier),
        }
    }

    fn describe_action(&self, name: &str, args: &Value) -> String {
        if self.is_builtin(name) {
            self.assistant.describe_action(name, args)
        } else {
            clawdot_core::session::describe_generic(name, args)
        }
    }

    fn returns_untrusted_text(&self, name: &str) -> bool {
        if self.is_builtin(name) {
            return self.assistant.returns_untrusted_text(name);
        }
        self.fixture(name).is_none_or(|(_, t)| t.untrusted)
    }

    fn expected_slow(&self, name: &str) -> bool {
        self.fixture(name).is_some_and(|(_, t)| t.slow)
    }

    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        self.executed.push(Executed {
            tool: name.to_owned(),
            args: args.clone(),
        });
        if self.is_builtin(name) {
            return self.assistant.call_tool(name, args).await;
        }
        let Some((_, t)) = self.fixture(name) else {
            return Err(format!("no fixture for `{name}`"));
        };
        let (hang, cancels, secs) = (t.hang, t.cancels_turn, t.duration_secs);
        self.clock.set(self.clock.now() + secs);
        if cancels {
            self.cancel.cancel();
        }
        if hang {
            std::future::pending::<()>().await;
        }
        let Some((i, t)) = self.fixture(name) else {
            return Err(format!("no fixture for `{name}`"));
        };
        let n = self.calls.get(i).copied().unwrap_or(0);
        let out = if let Some(e) = &t.error {
            Err(e.clone())
        } else if !t.results.is_empty() {
            Ok(t.results
                .get(n)
                .or_else(|| t.results.last())
                .cloned()
                .unwrap_or_default())
        } else {
            Ok(t.result.clone().unwrap_or_else(|| "ok".to_owned()))
        };
        if let Some(c) = self.calls.get_mut(i) {
            *c += 1;
        }
        out
    }
}
