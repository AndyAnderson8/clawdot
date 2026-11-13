//! Ending a turn, and settling what a batch of calls left for it.

use super::*;

impl<H: ToolHost> Turn<'_, H> {
    /// End the turn because the user cancelled it: discard what was held, record it, and say what was and was
    /// not done (REQ-AST-022).
    pub(in crate::session::gate) fn cancelled(
        &mut self,
        not_run: &[String],
        sent: Option<&str>,
    ) -> TurnResult {
        let mut discarded = false;
        if let Some(p) = self.conv.pending.take() {
            self.resolved(&p, decision::CANCELLED, "the user cancelled");
            self.conv.set_recent(p.recent, "cancelled");
            discarded = true;
        }
        self.conv.clarify = None;
        self.conv.forget_all_asked = None;
        self.note_act(&Act {
            tool: "turn",
            tier: None,
            args: &Value::Null,
            decision: decision::CANCELLED,
            held_id: None,
            outcome: None,
            note: Some("the user cancelled the turn"),
        });
        let mut r = take(&mut self.result);
        r.end = TurnEnd::Stopped(Stop::Cancelled);
        let mut speech = stop_speech(Stop::Cancelled, &self.cfg, &self.st.steps, not_run, sent);
        if discarded {
            speech.push_str(" I dropped the action I was about to ask you about.");
        }
        finish_with(self.conv, r, &speech)
    }

    /// End the turn at a limit.
    pub(in crate::session::gate) fn stopped(&mut self, why: Stop) -> TurnResult {
        stopped_turn(
            self.conv,
            take(&mut self.result),
            why,
            &self.cfg,
            &self.st,
            &[],
        )
    }

    /// End the turn with words the host chose.
    pub(in crate::session::gate) fn finish(&mut self, speech: &str) -> TurnResult {
        finish_with(self.conv, take(&mut self.result), speech)
    }

    /// End a turn on a call that was refused for being too long to read back. It counts as a stop for a failed
    /// call, not as an answer, so a caller that waits for the model to speak (the realtime voice) is told to
    /// say the host's words.
    pub(in crate::session::gate) fn finish_too_long(&mut self, tool: &str) -> TurnResult {
        let mut r = take(&mut self.result);
        r.end = TurnEnd::Stopped(Stop::Failures);
        finish_with(self.conv, r, &too_long_speech(tool))
    }

    /// End the turn because calls keep failing (REQ-AST-025).
    pub(in crate::session::gate) fn failed(&mut self) -> TurnResult {
        failed_turn(self.conv, take(&mut self.result), &self.st)
    }

    /// What follows a batch of calls: cancel, a question for the user, a held action to ask about, a limit,
    /// or calls that keep failing. `Some` ends the turn; `None` means the turn may go on.
    pub(in crate::session::gate) fn settle(&mut self, d: Dispatched) -> Option<TurnResult> {
        let Dispatched {
            mut held,
            ask,
            limit_hit,
            cancelled,
            sent,
            not_run,
        } = d;
        let mono = self.mono;
        if cancelled || self.ctl.cancel.is_cancelled() {
            if let Some(h) = held.take() {
                self.resolved(&h, decision::CANCELLED, "the user cancelled");
                self.conv.set_recent(h.recent, "cancelled");
            }
            return Some(self.cancelled(&not_run, sent.as_deref()));
        }
        if let Some(q) = ask {
            self.st.steps.push((ASK_USER.to_owned(), How::Asked));
            self.conv.clarify = Some(Clarify { asked_at: mono });
            self.result.end = TurnEnd::Asked;
            self.result.expects_reply = true;
            return Some(self.finish(&q));
        }
        if let Some(h) = held {
            // The host asks, in words built from the stored arguments, and the turn ends.
            let speech = self.question_about(&h);
            if let Some(old) = self.conv.pending.replace(h) {
                self.resolved(&old, decision::DECLINED, "replaced by a newer request");
                self.conv.set_recent(old.recent, "declined");
            }
            self.result.expects_reply = true;
            self.result.pending = true;
            return Some(self.finish(&speech));
        }
        if let Some(stop) = limit_hit {
            return Some(stopped_turn(
                self.conv,
                take(&mut self.result),
                stop,
                &self.cfg,
                &self.st,
                &not_run,
            ));
        }
        if self.st.fail_streak >= MAX_FAILURES_IN_ROW {
            return Some(failed_turn(self.conv, take(&mut self.result), &self.st));
        }
        None
    }

    /// The words the host asks with about a held action, built from its stored description: after text from
    /// outside the user is told to check it, and a turn the user did not start only suggests it.
    fn question_about(&self, held: &PendingAction) -> String {
        let what = held.description.trim_end_matches('.');
        if self.by_user && self.conv.tainted {
            format!(
                "{what}. This followed text from outside, such as a web page or an alert, so check it. Yes or no?"
            )
        } else if self.by_user {
            format!("{what}. Yes or no?")
        } else {
            format!("Suggested action: {what}. Say yes if you want that, or no to drop it.")
        }
    }

    /// End a turn whose calls came from somewhere other than a model round that could answer next: the words
    /// are what an empty model reply would have been (REQ-AST-025), "Done." unless the last thing was a
    /// failure.
    pub(in crate::session::gate) fn close_after_calls(&mut self) -> TurnResult {
        if let Some(tool) = self.st.too_long.take() {
            return self.finish_too_long(&tool);
        }
        if self.st.last_fail.is_some() {
            return failed_turn(self.conv, take(&mut self.result), &self.st);
        }
        self.result.speech = "Done.".to_owned();
        self.conv
            .history
            .push(Message::assistant(self.result.speech.clone()));
        self.conv.trim();
        take(&mut self.result)
    }
}
