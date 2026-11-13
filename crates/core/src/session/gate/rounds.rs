//! The model loop of a turn, as phases of [`Turn`]: what the host reads from the user's own words before any
//! model is asked, then rounds of asking the model (with the fallback model), and either finishing on its
//! words or dispatching its calls through the gate and settling what they left. Being a child of the gate
//! module, it works on the turn's private state; `run_turn_from` sees none of it.

use super::*;

impl<H: ToolHost> Turn<'_, H> {
    /// Run the turn to its end: the host's own answers first, then rounds with the model.
    pub(in crate::session) async fn run<B: ChatBackend>(
        &mut self,
        backend: &B,
        user_text: &str,
    ) -> TurnResult {
        if self.ctl.cancel.is_cancelled() {
            return self.cancelled(&[], None);
        }
        if let Some(r) = self.answer_before_model(user_text).await {
            return r;
        }
        let mut round = 0;
        loop {
            if let Some(r) = self.limit_before_round(round) {
                return r;
            }
            round += 1;
            self.st.spoke_round = false;
            if let Some(r) = self.round(backend, round).await {
                return r;
            }
        }
    }

    /// What the host answers itself, from the user's words, without asking the model: a question it asked,
    /// a note command, the answer to a held action (REQ-SEC-011, REQ-SEC-017). `Some` ends the turn.
    async fn answer_before_model(&mut self, user_text: &str) -> Option<TurnResult> {
        if let Some(r) = self.answer_first(user_text).await {
            return Some(r);
        }
        if !self.by_user {
            return None;
        }
        // A question the assistant asked is answered by this turn; "never mind" drops it.
        let (now, mono) = (self.now, self.mono);
        let asked = self
            .conv
            .clarify
            .take_if(|c| mono - c.asked_at <= CONFIRM_WINDOW_SECS)
            .is_some();
        if asked && is_no(self.cfg.lang, user_text) {
            return Some(self.finish("Okay, never mind."));
        }
        // Notes are the user's to change, by their own words, and never go through the model (REQ-SEC-017).
        if let Some(cmd) = notes::parse(self.cfg.lang, user_text) {
            let speech = run_note_command(self.conv, cmd, now, mono);
            return Some(self.finish(&speech));
        }
        None
    }

    /// The checks before asking the model again: cancel, the round and time limits, the daily caps.
    fn limit_before_round(&mut self, round: usize) -> Option<TurnResult> {
        if self.ctl.cancel.is_cancelled() {
            return Some(self.cancelled(&[], None));
        }
        if round >= self.cfg.max_tool_loops {
            return Some(self.stopped(Stop::Rounds));
        }
        if self.st.over_deadline(self.ctl, &self.cfg) {
            return Some(self.stopped(Stop::Deadline));
        }
        if let Err(limit) = self.usage.check(self.day) {
            self.result.speech = limit.to_string();
            self.result.end = TurnEnd::Limit(limit);
            return Some(take(&mut self.result));
        }
        None
    }

    /// One round: ask the model, then finish on its words or run its calls. `Some` ends the turn; `None`
    /// means the calls were settled and another round may follow.
    async fn round<B: ChatBackend>(&mut self, backend: &B, round: usize) -> Option<TurnResult> {
        let mut tools = offered(&*self.host, &self.cfg);
        tools.extend(pseudo_tools(self.conv, self.by_user));
        let mut completion = match self.ask_model(backend, &tools).await {
            Ok(c) => c,
            Err(ended) => return Some(ended),
        };
        if completion.tool_calls.is_empty() {
            return Some(self.finish_on_words(&completion));
        }
        self.conv.unique_call_ids(&mut completion.tool_calls);
        self.conv.history.push(Message::assistant_calls(
            completion.text.clone(),
            completion.tool_calls.clone(),
        ));
        let spaced = self
            .st
            .last_spoke
            .is_none_or(|x| self.ctl.now() - x >= PROGRESS_GAP_SECS);
        self.speak_ahead(round, &completion, spaced);
        let dispatched = self
            .dispatch(Some(backend), &completion.tool_calls, &tools, spaced)
            .await;
        self.settle(dispatched)
    }

    /// Ask the model, and the fallback model if the first fails. `Err` is the finished turn: the user
    /// cancelled while a request was in flight, or no model answered.
    async fn ask_model<B: ChatBackend>(
        &mut self,
        backend: &B,
        tools: &[ToolSpec],
    ) -> Result<Completion, TurnResult> {
        let day = self.day;
        let mut models = vec![self.cfg.model.clone()];
        models.extend(self.cfg.fallback_model.clone());
        let mut last_err = None;
        for model in models {
            if self.usage.check(day).is_err() {
                break;
            }
            let req = self
                .conv
                .request(&model, tools.to_vec(), self.cfg.max_tokens);
            self.result.model_requests += 1;
            // A model request in flight is abandoned if the user says stop.
            let outcome = tokio::select! {
                r = backend.complete(&req) => Some(r),
                () = self.ctl.cancel.cancelled() => None,
            };
            match outcome {
                None => {
                    self.usage.record_abandoned(day, &req);
                    return Err(self.cancelled(&[], None));
                }
                Some(Ok(c)) => {
                    self.usage.record_reply(day, &req, &c);
                    return Ok(c);
                }
                Some(Err(e)) => {
                    self.usage.record_request(day);
                    last_err = Some(e);
                }
            }
        }
        self.result.speech = "Sorry, I couldn't reach the language model.".to_owned();
        self.result.end = last_err.map_or(TurnEnd::Limit(LimitReached::Requests), TurnEnd::Failed);
        Err(take(&mut self.result))
    }

    /// End the turn on a reply without calls: its words are the answer.
    fn finish_on_words(&mut self, completion: &Completion) -> TurnResult {
        // The last thing was a call too long to read back: whatever the model says, the user hears that
        // nothing was done.
        if let Some(tool) = self.st.too_long.take() {
            return self.finish_too_long(&tool);
        }
        let text = completion.text.trim();
        if text.is_empty() && self.st.last_fail.is_some() {
            // The last thing that happened was a failure and the model said nothing: never "Done."
            return self.failed();
        }
        self.result.speech = if text.is_empty() {
            "Done.".to_owned()
        } else {
            text.to_owned()
        };
        self.conv
            .history
            .push(Message::assistant(self.result.speech.clone()));
        self.conv.trim();
        take(&mut self.result)
    }

    /// Words the model has for the user alongside its calls are the plan (first round) or a progress remark;
    /// with several calls and no words the host writes the plan (REQ-AST-020).
    fn speak_ahead(&mut self, round: usize, completion: &Completion, spaced: bool) {
        let words = first_sentence(&completion.text);
        if round == 1 && self.cfg.plan_speech {
            if !words.is_empty() {
                say_early(self.ctl, &mut self.result, &mut self.st, words);
            } else if completion.tool_calls.len() >= 3 {
                say_early(
                    self.ctl,
                    &mut self.result,
                    &mut self.st,
                    plan_from_calls(&completion.tool_calls),
                );
            }
        } else if round > 1 && self.cfg.progress_speech && !words.is_empty() && spaced {
            say_early(self.ctl, &mut self.result, &mut self.st, words);
        }
    }
}
