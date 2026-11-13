//! The user's own answers (to "forget all my notes?" and to a held action), and the release of a held action.

use super::*;

impl<H: ToolHost> Turn<'_, H> {
    /// The answers the host reads from the user's own words before anything else: to "forget all my notes?"
    /// and to a held action. `Some` ends the turn.
    pub(in crate::session::gate) async fn answer_first(
        &mut self,
        user_text: &str,
    ) -> Option<TurnResult> {
        if let Some(r) = self.answer_forget_all(user_text) {
            return Some(r);
        }
        self.answer_held(user_text).await
    }

    /// Whether an action is waiting for the user's answer and its window is still open. One whose window has
    /// run out is dropped here, so it does not stand in the way of a new one.
    pub(in crate::session::gate) fn held_and_fresh(&mut self) -> bool {
        self.drop_held_if_stale();
        self.conv.pending.is_some()
    }

    /// Drop a held action whose window has run out, for a turn that cannot answer it (the calls of a model
    /// outside the turn loop). One still inside its window is left alone.
    pub(in crate::session::gate) fn drop_held_if_stale(&mut self) {
        let mono = self.mono;
        if let Some(old) = self
            .conv
            .pending
            .take_if(|p| mono - p.proposed_at > CONFIRM_WINDOW_SECS)
        {
            self.resolved(&old, decision::EXPIRED, "no answer in time");
            self.conv.set_recent(old.recent, "expired");
        }
    }

    /// The user's answer to "forget all my notes?" is read here, by the host. `Some` ends the turn.
    pub(in crate::session::gate) fn answer_forget_all(
        &mut self,
        user_text: &str,
    ) -> Option<TurnResult> {
        let (by_user, mono) = (self.by_user, self.mono);
        let conv = &mut *self.conv;
        let result = &mut self.result;
        if by_user && let Some(at) = conv.forget_all_asked.take() {
            if mono - at <= CONFIRM_WINDOW_SECS && is_yes(self.cfg.lang, user_text) {
                let speech = match conv.notes.as_ref().map(|n| n.clear()) {
                    Some(Ok(n)) => format!(
                        "Okay, I forgot all {n} note{}.",
                        if n == 1 { "" } else { "s" }
                    ),
                    Some(Err(e)) => e.spoken(),
                    None => notes::NoteError::Off.spoken(),
                };
                return Some(finish_with(conv, take(result), &speech));
            }
            if is_no(self.cfg.lang, user_text) {
                return Some(finish_with(conv, take(result), "Okay, I'll keep them."));
            }
        }
        None
    }

    /// The user's answer to a held action is read here, by the host, from the user's own words. It is the
    /// only way a held action runs (REQ-SEC-011). An event is never an answer. `Some` ends the turn.
    pub(in crate::session::gate) async fn answer_held(
        &mut self,
        user_text: &str,
    ) -> Option<TurnResult> {
        if !self.by_user {
            return None;
        }
        let held = self.conv.pending.take()?;
        if is_yes(self.cfg.lang, user_text) && self.mono - held.proposed_at > CONFIRM_WINDOW_SECS {
            let speech = "That took too long, so I dropped it. Please ask again.";
            return Some(self.drop_held(
                held,
                HeldOutcome::Expired,
                decision::EXPIRED,
                "answered too late",
                "expired",
                speech,
            ));
        }
        if is_no(self.cfg.lang, user_text) {
            let speech = "Okay, I won't do that.";
            return Some(self.drop_held(
                held,
                HeldOutcome::Declined,
                decision::DECLINED,
                "the user said no",
                "declined",
                speech,
            ));
        }
        if is_yes(self.cfg.lang, user_text) {
            return self.release_held(held).await;
        }
        // Anything else is an unrelated request: the held action is dropped and the turn goes on.
        self.resolved(&held, decision::DECLINED, "unrelated request");
        self.conv.set_recent(held.recent, "declined");
        self.result.held = Some(HeldOutcome::Declined);
        None
    }

    /// End the turn by dropping a held action without running it: the audit record, the recent-actions line,
    /// and the words to say.
    pub(in crate::session::gate) fn drop_held(
        &mut self,
        held: PendingAction,
        outcome: HeldOutcome,
        how: &str,
        why: &str,
        recent: &str,
        speech: &str,
    ) -> TurnResult {
        self.resolved(&held, how, why);
        self.conv.set_recent(held.recent, recent);
        self.result.held = Some(outcome);
        self.finish(speech)
    }

    /// The user said yes in time: run the stored action, put the call and its result into the history as a
    /// model's call would have been, and end the turn only if the user cancelled meanwhile.
    pub(in crate::session::gate) async fn release_held(
        &mut self,
        held: PendingAction,
    ) -> Option<TurnResult> {
        let offered_now = offered(&*self.host, &self.cfg)
            .into_iter()
            .any(|t| t.name == held.tool);
        let outcome = if offered_now {
            let gate = self.turn.as_ref().map_or(Gate::Run, |t| t.gate(held.tier));
            // Counted as a call only because something runs, as `admit` does for a model's call.
            self.result.tool_calls += 1;
            let ex = self
                .execute(
                    &Call {
                        name: &held.tool,
                        args: &held.args,
                        tier: held.tier,
                        held_id: held.held_id,
                    },
                    gate,
                )
                .await;
            self.conv.set_recent(held.recent, status_of(ex.kind));
            self.result.held = Some(if ex.kind == Kind::Ok {
                HeldOutcome::Ran
            } else {
                HeldOutcome::Failed
            });
            match ex.kind {
                Kind::Ok => self.st.record(&held.tool, How::Done, held.tier),
                Kind::Failed => self.st.record(&held.tool, How::Failed, held.tier),
                Kind::TimedOut => self.st.record(&held.tool, How::TimedOut, held.tier),
                Kind::Cancelled | Kind::NotRun => {}
            }
            ex.text
        } else {
            self.resolved(&held, decision::INVALID, "tool no longer offered");
            self.conv.set_recent(held.recent, "no longer available");
            self.result.held = Some(HeldOutcome::Gone);
            "Error: that tool is no longer available.".to_owned()
        };
        let id = self.conv.host_call_id();
        let call = ToolCall {
            id: id.clone(),
            name: held.tool.clone(),
            arguments: held.args.to_string(),
        };
        self.conv
            .history
            .push(Message::assistant_calls(String::new(), vec![call]));
        self.conv
            .history
            .push(Message::tool_result(id, clip(&outcome)));
        if self.ctl.cancel.is_cancelled() {
            let sent = (held.tier >= Tier::Costly).then_some(held.tool.as_str());
            return Some(self.cancelled(&[], sent));
        }
        None
    }
}
