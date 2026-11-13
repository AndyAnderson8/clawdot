//! Changes asked for in the web UI.

use super::App;
use crate::app::{Fail, Job, Op};
use crate::extensions::ExtOp;
use crate::runtime::apply_live;

impl App {
    /// Carry out one change the portal asked for and answer it. Everything here is quick and local except
    /// adding an MCP connection: the loop checks the request and starts a task for the network half, and the
    /// answer is sent when that task's result comes back through [`App::on_mcp_connected`] (ADR-0021).
    pub async fn on_portal_job(&mut self, job: Job) {
        let Job { op, reply } = job;
        let answer = match op {
            Op::Ext(ExtOp::McpAdd(v)) => {
                let brain = &mut self.brain;
                let core = &self.core;
                match brain
                    .ext
                    .mcp_prepare(&v, &brain.mcp, brain.connects.portal_len(), |v| {
                        core.env().get(v)
                    }) {
                    Ok(pending) => {
                        brain.connects.start(pending, reply);
                        return;
                    }
                    Err(f) => Err(f),
                }
            }
            // Removing a connection that is still connecting stops it; that is the whole removal.
            Op::Ext(ExtOp::McpDelete(name)) if self.brain.connects.cancel(&name) => {
                Ok(self.brain.listing())
            }
            // A saved source that is still connecting is removed like any other; its connection stops with it.
            Op::Ext(ExtOp::McpDelete(name)) => {
                let removed = self.sync_op(Op::Ext(ExtOp::McpDelete(name.clone()))).await;
                if removed.is_ok() {
                    self.brain.connects.forget_saved(&name);
                }
                removed
            }
            // A change to the timers is answered once it is on disk, and answered as a failure if it could not be
            // written: it is made and will ring, but the page must not say it is saved when it is not.
            op @ Op::Sched(_) => {
                let answer = self.sync_op(op).await;
                match self.brain.assistant.take_save() {
                    Some(saved) if answer.is_ok() => {
                        tokio::spawn(async move {
                            let _ = reply.send(match saved.wait().await {
                                Ok(()) => answer,
                                Err(e) => Err(Fail::NotSaved(
                                    "The change was made, but it could not be saved, so it will be lost if the assistant restarts.".into(),
                                    e.disk_full,
                                )),
                            });
                        });
                        return;
                    }
                    _ => answer,
                }
            }
            op => self.sync_op(op).await,
        };
        let _ = reply.send(answer);
    }

    /// The changes that need no network: done on the spot.
    async fn sync_op(&mut self, op: Op) -> Result<serde_json::Value, Fail> {
        match op {
            Op::Sched(op) => self.brain.assistant.admin_sched(op),
            Op::Settings(live) => {
                apply_live(&mut self.brain, &self.core, &live, &mut self.applied);
                Ok(serde_json::json!({ "applied": true }))
            }
            Op::Rekey => {
                // Make the next comparison see a change, so the client is built again with the new key.
                let live = self.applied.clone();
                self.applied
                    .insert("llm_key_env".into(), serde_json::Value::Null);
                apply_live(&mut self.brain, &self.core, &live, &mut self.applied);
                Ok(serde_json::json!({ "applied": true }))
            }
            Op::Ext(op) => {
                let r = self.brain.ext_apply(op, self.core.env()).await;
                if let Ok(listing) = &r {
                    self.core.set_ui_tools(listing.clone());
                }
                r
            }
        }
    }
}
