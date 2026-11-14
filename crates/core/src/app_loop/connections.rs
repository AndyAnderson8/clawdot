//! MCP connections made beside the loop (`connects`): what the loop does when one reports.

use super::App;
use crate::connects::{Done, Source};
use crate::extensions::ToolSets;
use std::sync::Arc;

impl App {
    /// A connection task reported. For a portal request: add the server if it is still wanted, and answer the
    /// request. For a source saved earlier (connected at start-up): record how it went; it stays in the list
    /// either way, and a failure is printed and shown as the source's error. A result whose connection was
    /// cancelled or replaced meanwhile is dropped (see `connects`).
    pub fn on_mcp_connected(&mut self, done: Done) {
        let brain = &mut self.brain;
        let core = &self.core;
        let Some(source) = brain.connects.finish(&done) else {
            return;
        };
        let reply = match source {
            Source::Portal(reply) => reply,
            Source::Saved => {
                self.settle_saved(done);
                return;
            }
        };
        let answer = match done.tools {
            Err(why) => Err(crate::app::Fail::Invalid(why)),
            Ok(tools) => {
                let applied = brain.ext.mcp_commit(
                    &done.pending,
                    tools,
                    ToolSets::new(
                        Arc::make_mut(&mut brain.http_tools),
                        Arc::make_mut(&mut brain.mcp),
                        &brain.builtin,
                        &|n: &str| core.env().get(n),
                    ),
                );
                brain.ext_view = Arc::new(brain.ext.snapshot());
                applied.map(|()| {
                    let listing = brain.listing();
                    core.set_ui_tools(listing.clone());
                    listing
                })
            }
        };
        let _ = reply.send(answer);
    }

    /// A source saved earlier reported: record how the connection went. It stays in the list either way.
    fn settle_saved(&mut self, done: Done) {
        let brain = &mut self.brain;
        let core = &self.core;
        let mut reserved = brain.builtin.clone();
        reserved.extend(brain.http_tools.specs().into_iter().map(|t| t.name));
        let name = done.pending.name().to_owned();
        let listed = done.tools.as_ref().ok().map(Vec::len);
        // Only what the administrator approved is offered; a server never approved (saved before
        // approvals existed, or defined in the configuration file) is approved as found, once.
        let approved = brain.ext.approved_tools(&name);
        let warnings = Arc::make_mut(&mut brain.mcp).settle(
            done.pending.name(),
            done.tools,
            &reserved,
            approved.as_ref(),
        );
        for w in warnings {
            eprintln!("clawdot: {w}");
        }
        if let Some(n) = listed {
            println!("clawdot: mcp server `{name}` connected ({n} tools listed)");
        }
        if approved.is_none() {
            match brain.ext.pin_first_connect(&name, &brain.mcp) {
                Ok(true) => println!(
                    "clawdot: mcp server `{name}`: its tools are approved as found (first connect); later changes wait for review"
                ),
                Ok(false) => {}
                Err(e) => {
                    eprintln!("clawdot: mcp server `{name}`: could not record its tools: {e:?}");
                }
            }
        }
        core.set_ui_tools(brain.listing());
    }

    /// Serve the connection results until none is outstanding. For the one-shot command: with no loop to serve
    /// meanwhile, wait for the sources to answer first, as the single turn should have their tools.
    pub async fn wait_for_connections(&mut self) {
        while self.brain.is_connecting() {
            let Some(done) = self.sources.mcp.recv().await else {
                break;
            };
            self.on_mcp_connected(done);
        }
    }
}
