//! How much harm a call can do, and the two traits a host implements: where completions come from and
//! which tools a turn can use (REQ-SEC-010).

use super::describe_generic;
use crate::llm::{ChatRequest, Completion, LlmError, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;

/// How much harm a tool call can do (REQ-SEC-010), lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Reads only; nothing leaves ClawDot and nothing outside changes.
    ReadOnly,
    /// Changes ClawDot's own state or something trivial that one command undoes: a timer, a pause.
    Reversible,
    /// Visible to others, costs money or quota, or changes a remote system. Held for the user's yes.
    Costly,
    /// Loses data or access. Held for the user's yes and not offered unless the administrator enables it.
    Destructive,
}

impl Tier {
    /// The name used in configuration and in the portal.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Reversible => "reversible",
            Self::Costly => "costly",
            Self::Destructive => "destructive",
        }
    }
}

/// A call's tier, and the tier it would have had without an administrator's override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Risk {
    /// The tier in force.
    pub tier: Tier,
    /// The conservative tier for this kind of tool before any override (costly for HTTP and MCP tools).
    /// Untrusted text in a turn makes a lowered tool behave at this tier again (REQ-SEC-012).
    pub baseline: Tier,
}

impl Risk {
    /// A tier that was not lowered by anyone.
    #[must_use]
    pub fn fixed(tier: Tier) -> Self {
        Self {
            tier,
            baseline: tier,
        }
    }

    /// A tier the administrator set, over a conservative `baseline`.
    #[must_use]
    pub fn set(tier: Tier, baseline: Tier) -> Self {
        Self {
            tier,
            baseline: baseline.max(tier),
        }
    }

    /// The risk to act on. When untrusted text is in the turn, a tool whose tier was lowered below its
    /// baseline is treated at that baseline again, including one lowered to read-only: injected text could
    /// otherwise make the model call a "read-only" fetch or search tool with the user's data in its
    /// arguments, unasked. A tool whose baseline is itself read-only (a built-in read) is unaffected
    /// (REQ-SEC-012).
    pub(super) fn under(self, tainted: bool) -> Self {
        if tainted {
            Self {
                tier: self.tier.max(self.baseline),
                baseline: self.baseline,
            }
        } else {
            self
        }
    }

    /// Whether the call must wait for the user's yes.
    pub(super) fn needs_hold(self) -> bool {
        self.tier >= Tier::Costly
    }
}

/// Where completions come from. Implemented over HTTP in [`crate::transport`] and by fakes in tests.
pub trait ChatBackend {
    /// Send a request and return the finished response.
    fn complete(
        &self,
        req: &ChatRequest,
    ) -> impl Future<Output = Result<Completion, LlmError>> + Send;
}

/// The tools a turn can use.
pub trait ToolHost {
    /// Tools to offer the model.
    fn tool_specs(&self) -> Vec<ToolSpec>;
    /// How risky a call is (REQ-SEC-010). Declared by whoever wrote the tool host or configured the tool,
    /// never read from the model's words or from a server's own hints. A tool that does not declare one is
    /// costly. A tool whose risk depends on its arguments (an `ack` versus an `instruct` reply) looks at
    /// `args`; the tool list offered to the model is filtered with `Value::Null`.
    fn risk(&self, _name: &str, _args: &Value) -> Risk {
        Risk::fixed(Tier::Costly)
    }
    /// What to say to the user when this call is held: a plain sentence built from the arguments, without a
    /// question (the host adds that). It must state every argument that changes what happens, in full, and
    /// must not rely on anything the model wrote besides the arguments. One longer than
    /// [`MAX_READ_BACK_CHARS`](super::MAX_READ_BACK_CHARS) is not held: the call is refused instead (REQ-SEC-011).
    fn describe_action(&self, name: &str, args: &Value) -> String {
        describe_generic(name, args)
    }
    /// Drop from the model's arguments any field that only the host may set (see [`ToolHost::pin_args`]).
    /// Called on every call as it arrives, before the tier is judged, so nothing the model writes under such a
    /// name reaches the read-back, the held action or the tool.
    fn clean_args(&self, _name: &str, _args: &mut Value) {}
    /// Fix the arguments of a call that is about to be held or kept as a proposal, before its read-back is
    /// built and before it is stored. A tool whose effect depends on state that can change while the user
    /// thinks (which agent event a reply goes to) has the host write that state into the arguments here, so
    /// the user hears the destination and the yes releases the call to that destination and no other. The
    /// values come from the host's own state, never from the model.
    fn pin_args(&mut self, _name: &str, args: Value) -> impl std::future::Future<Output = Value> {
        async move { args }
    }
    /// Whether this tool's results are text written by someone else (a web page, an MCP server, an agent
    /// event). Such a result makes the rest of the conversation "tainted" (REQ-SEC-012). A tool that does
    /// not say is assumed to return untrusted text.
    fn returns_untrusted_text(&self, _name: &str) -> bool {
        true
    }
    /// Whether a call to this tool is expected to take more than about three seconds (a network request), so
    /// the host says a short phrase before it (REQ-AST-020). Local tools say no.
    fn expected_slow(&self, _name: &str) -> bool {
        false
    }
    /// Called once at the start of every user turn with the user's own words, so a tool can tell
    /// which turn it is running in and what the user actually said (REQ-AST-015).
    fn begin_turn(&mut self, _user_text: &str) {}
    /// Run a tool. `Err` text is shown to the model, not hidden.
    ///
    /// # Errors
    /// A message describing why the tool failed.
    fn call_tool(
        &mut self,
        name: &str,
        args: &Value,
    ) -> impl std::future::Future<Output = Result<String, String>>;
}
