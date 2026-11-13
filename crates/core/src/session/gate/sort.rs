//! Phase 3 of a call: what the host decides by its tier (refuse, propose, block, hold, or run), and how an action is kept for the user.

use super::*;

impl<H: ToolHost> Turn<'_, H> {
    /// Phase 3: what the host decides for a valid call, by its tier. It is refused (destructive and not
    /// offered), kept as a proposal (the turn is not the user's), blocked (dry-run or a cap), held for the
    /// user's yes, or run.
    pub(in crate::session::gate) async fn sort_by_tier<B: ChatBackend>(
        &mut self,
        batch: &Batch<'_, B>,
        call: &ToolCall,
        args: Value,
        d: &mut Dispatched,
    ) -> String {
        let risk = self.host.risk(&call.name, &args).under(self.conv.tainted);
        let gate = self.turn.as_ref().map_or(Gate::Run, |t| t.gate(risk.tier));
        // An action of this batch is already held, or one from an earlier batch is still waiting.
        let batch_holds = d.held.is_some();
        if risk.tier == Tier::Destructive && !self.cfg.allow_destructive {
            self.refuse_destructive(call, &args, risk.tier)
        } else if !self.by_user
            && risk.tier != Tier::ReadOnly
            && (batch_holds || self.held_and_fresh())
        {
            // The user is deciding on an action already. A turn that outside text started must not put its own
            // proposal in that action's place, where a yes meant for the first would run the second
            // (REQ-SEC-011, REQ-SEC-012).
            self.note_skipped_for_waiting(call, &args, risk.tier);
            "Not run and not proposed: an action is already waiting for the user's answer. Say in your reply what you would suggest."
                .to_owned()
        } else if !self.by_user && risk.tier != Tier::ReadOnly {
            // Nobody is here to ask: keep it as a proposal for the user's next turn.
            self.keep(call, args, risk.tier, &PROPOSE, d).await
        } else if gate != Gate::Run {
            // Dry-run or a daily cap: nothing is asked and nothing runs.
            self.block_call(call, &args, risk.tier, gate)
        } else if risk.needs_hold() && (batch_holds || self.conv.pending.is_some()) {
            // An action is already waiting for the user's yes or no, held earlier in this batch or by an earlier
            // call. A second one must not replace it and a call must not cancel it: only the user's own words
            // end it (REQ-SEC-011). A realtime session reaches this across batches, because its model keeps
            // calling tools while the question is open; a turn of the model loop reaches it within a batch.
            self.note_skipped_for_waiting(call, &args, risk.tier);
            "Not run: an action is already waiting for the user's answer, so this one was not held. Do not retry it until the user has answered; they will say yes or no."
                .to_owned()
        } else if risk.needs_hold() {
            self.keep(call, args, risk.tier, &HOLD, d).await
        } else {
            self.run_call(batch, call, &args, risk.tier, d).await
        }
    }

    /// A destructive call when destructive tools are not offered: refused as if the tool did not exist.
    pub(in crate::session::gate) fn refuse_destructive(
        &self,
        call: &ToolCall,
        args: &Value,
        tier: Tier,
    ) -> String {
        self.note_act(&Act {
            tool: &call.name,
            tier: Some(tier),
            args,
            decision: decision::INVALID,
            held_id: None,
            outcome: None,
            note: Some("destructive tools are not offered"),
        });
        format!("Error: there is no tool called `{}`.", call.name)
    }

    /// Record a call that was passed over because another action waits for the user's answer.
    pub(in crate::session::gate) fn note_skipped_for_waiting(
        &self,
        call: &ToolCall,
        args: &Value,
        tier: Tier,
    ) {
        self.note_act(&Act {
            tool: &call.name,
            tier: Some(tier),
            args,
            decision: decision::SKIPPED,
            held_id: None,
            outcome: None,
            note: Some("another action is waiting for the user's answer"),
        });
    }

    /// Dry-run or a daily cap: the recent-actions line, then what the model is told.
    pub(in crate::session::gate) fn block_call(
        &mut self,
        call: &ToolCall,
        args: &Value,
        tier: Tier,
        gate: Gate,
    ) -> String {
        self.conv
            .note_recent(&call.name, Some(tier), args, "not run");
        self.blocked(
            &Call {
                name: &call.name,
                args,
                tier,
                held_id: None,
            },
            gate,
        )
    }

    /// Keep a call for the user to answer, as a hold or a proposal (`how`): the host pins its arguments, the
    /// action is built and recorded, and the batch ends up holding it. What the model is told is `how.told`,
    /// or the refusal when the read-back is too long.
    pub(in crate::session::gate) async fn keep(
        &mut self,
        call: &ToolCall,
        args: Value,
        tier: Tier,
        how: &Kept,
        d: &mut Dispatched,
    ) -> String {
        let args = self.host.pin_args(&call.name, args).await;
        match self.keep_pending(call, &args, tier, how) {
            Ok(p) => {
                d.held = Some(p);
                how.told.to_owned()
            }
            Err(refusal) => refusal,
        }
    }

    /// Write the audit record for an action that is waiting for the user, and build it. The read-back must be
    /// short enough to say (REQ-SEC-011): if it is not, nothing is held and nothing runs, and the refusal
    /// (what the model is told) says how far over the limit it is.
    pub(in crate::session::gate) fn keep_pending(
        &mut self,
        call: &ToolCall,
        args: &Value,
        tier: Tier,
        how: &Kept,
    ) -> Result<PendingAction, String> {
        let description = self.host.describe_action(&call.name, args);
        let len = description.chars().count();
        if len > MAX_READ_BACK_CHARS {
            self.note_act(&Act {
                tool: &call.name,
                tier: Some(tier),
                args,
                decision: decision::INVALID,
                held_id: None,
                outcome: None,
                note: Some("too long to read back, so not held and not run"),
            });
            self.conv
                .note_recent(&call.name, Some(tier), args, "too long to read back");
            self.st.too_long = Some(call.name.clone());
            return Err(format!(
                "Not run, and nothing was held: reading this call back to the user would take {len} characters, and the most that can be read aloud for approval is {MAX_READ_BACK_CHARS}, so it is {} over. Shorten the values or split the work into smaller calls, then try again.",
                len - MAX_READ_BACK_CHARS
            ));
        }
        let id = self.note_act(&Act {
            tool: &call.name,
            tier: Some(tier),
            args,
            decision: how.decision,
            held_id: None,
            outcome: None,
            note: Some(how.why),
        });
        let mut p = PendingAction::new(&call.name, args.clone(), description, self.mono, tier);
        p.held_id = id;
        p.recent = Some(
            self.conv
                .note_recent(&call.name, Some(tier), args, how.recent),
        );
        self.st.too_long = None;
        Ok(p)
    }
}
