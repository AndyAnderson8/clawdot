# Speech latency: end of speech to first sound

The wait a listener feels is speech-to-text, then the model, then speech synthesis, then the first audio on the speaker. This page says what is built for the last step and designs the one before it. It serves REQ-VOI-004 (begin playback before the full response is generated).

## Built: the reply plays sentence by sentence (satellite session)

Before, a turn did `transcribe -> respond (whole reply) -> synthesise every sentence -> return`, and the session played only after all of it. For a reply of N sentences the first sound waited for N syntheses.

Now a turn ends when the reply *text* is complete. The session (`satellite/reply.rs`) then owns a `Reply`: the sentences still to say, at most one synthesis in flight, and a pace. It is one more arm of the event loop, beside the turn, the progress speech and the announcements.

1. The turn finishes. Progress phrases still come first. Effects (volume, stop, queued stream commands) are applied as before.
2. The first sentence is synthesised. Until its audio exists the dialog is still *thinking*: the thinking cue can still play and a stop phrase or the action button still works.
3. When it is ready the reply *starts*: the dialog moves to speaking (the cue ends, the microphone is ignored), music is paused, and `set-indicator speaking`, `audio-start` and the first chunks go out.
4. The next sentence is asked for, and its chunks go out on the same stream as soon as it is made. After the last one comes `audio-stop`. The satellite sends one `playback-done` after that, exactly as for a reply made all at once, and that opens the follow-up window and resumes music, once.

**No protocol change.** `audio-start` ... `audio-chunk`* ... `audio-stop` already allows audio to arrive over time (the realtime path does the same); `playback-done` is defined as the end of the last `audio-stop`ped reply. A satellite that buffers a whole reply until `audio-stop` (the ESPHome client in `esphome/client.rs`) plays correctly but gains nothing.

**Bounds.** The core holds one sentence of audio at a time, while it is written to the connection, and one synthesis request in flight; nothing is queued. Pace: the next sentence is asked for only when the speaker is estimated (from the length of the audio sent, at the protocol's rate) to have `reply_lead` (2 s; a field of the satellite server's `Settings`, not a user setting) or less left to play. So the satellite holds about one sentence beyond the one playing, and a stop wastes at most about one synthesis. If synthesis takes longer than the lead, the listener hears a clean pause between sentences, never a restart. Before, the core held every sentence of the reply at once.

**Rules that still hold.**

| Rule | How |
|---|---|
| Half-duplex (REQ-VOI-012) | `on_reply_started` runs at the first sound and the dialog stays *speaking* until `playback-done`; the microphone and the wake word are ignored throughout. |
| Thinking cue (REQ-VOI-010) | Plays if the first sentence is slow; stops when the reply starts, not when it is complete. |
| Progress before the answer | `deliver_finished` still waits for progress speech; the reply starts only afterwards. |
| Music | Paused at the first sound, resumed (or the queued stream started) once, on `playback-done` after the last sentence. |
| Stop (REQ-VOI-020) | The action button or a stop phrase drops the `Reply`, with the synthesis in flight: if it had started, `stop-playback` is sent and the reply ends like a finished one; if not, nothing is played and the dialog goes idle. A `playback-done` that arrives before the last sentence also drops it. A wake word during the reply is still ignored (REQ-VOI-012). |
| Mute | As before: it changes what happens after the reply (no follow-up window); it does not cut speech already started. |
| Follow-up window, `expects_reply` | Opened once by `playback-done` after the last sentence; `expect_answer` is set when the reply starts. |
| Synthesis fails on sentence k > 1 | What was played stays played. The stream is closed with `audio-stop`, no error cue, the follow-up window opens as usual, and `clawdot: speech synthesis: <reason>` is logged. Failing on sentence 1: nothing was played, so the error cue, and stream commands that waited run now. |
| Write timeout, dead-peer tick | Unchanged: the reply arm is just another arm, and writes go through the same timed writer. |
| Realtime, announcements | Unchanged. |
| Console and `--voice-in` | Unchanged: `voice_turn` still returns all the audio. |

**Not covered.** A stop pressed *on the device* while a reply is streaming stops the device's own playback, but the device says nothing until the core's `audio-stop` arrives, so the core keeps synthesising until then (bounded by the pace to about one sentence ahead). A `clawdot.cancel-turn` message ([spec 0006](../specs/0006-device-layer-contract.md) section 8) would close that gap; it needs a protocol change and is not part of this step.

## Design only: speaking while the model is still writing

Today `ChatBackend::complete` returns a whole `Completion`. The first sentence could be synthesised while the model is still writing the rest.

**Where the seam goes.** `ChatBackend` gets a second, optional method, `complete_streaming(req, on_text)`, with a default that calls `complete` and reports the finished text at the end. `HttpChat` implements it with the existing `StreamAccumulator` (`llm.rs` already parses SSE and has `SentenceSplitter`). The turn loop (`session/gate/rounds.rs`, `ask_model`) passes a sink only on a round where speaking early is allowed. The sink sends *candidate* sentences down a channel to the satellite session, which today receives only the finished `TurnResult`.

**What may be spoken before the completion ends.** Text can be spoken early only if the round turns out to be a final answer: a reply with tool calls carries a plan or a progress remark, spoken by `speak_ahead` under its own rules, and the same words must not also be spoken as the answer. The provider does not say which kind it is until the first tool-call delta or `finish_reason` arrives. So:

- Hold the first sentence until it is long enough to be worth it *and* no tool-call delta has been seen. Models usually open a call before or immediately after a short lead-in, so a call seen first, or at any time before the first sentence is released, cancels streaming for that round and the text goes down the existing plan/progress path.
- Once a sentence has been released, a later tool-call delta in the same response is an anomaly (words, then calls): the spoken sentence is kept as the progress phrase (it is the same text `speak_ahead` would take as `first_sentence`) and not spoken again.
- Only the user-visible answer path streams. Rounds after tool results, where the final words follow calls, qualify; the round that issues calls does not.

**The host may replace the words.** `finish_on_words` can substitute text: the read-back refusal for a call too long to read back (`too_long`), "Done." for empty text, the failure account when the last call failed, limit and cancel messages. These decisions are known *before* the model answers (`too_long`, `last_fail`) or after it finishes (empty text). So: if `too_long` or `last_fail` is set, or the turn is at a limit, do not stream the round at all. For the rest, the host's final text equals the model's trimmed text, so what was spoken early is a prefix of what the final reply would say; the final `TurnResult.speech` then holds only the sentences not already spoken. If the final text ever differs from the streamed prefix (a trim that changes words, a post-filter), the early audio cannot be recalled, so the rule is: the host may not rewrite a streamed round. Any new rewrite rule must either run before streaming starts or mark the round non-streamable.

**Cancel.** The cancel token already abandons the model request in flight. The same select also drops the streaming sink; sentences not yet released are never spoken; the sentence on the speaker is stopped as in the built behaviour above. Usage is recorded as abandoned, as today.

**Untrusted text.** Streamed text is model output like any other: it is spoken, never interpreted, and goes through the same limits (length, `MAX_STREAMED`). Nothing in a half-finished stream can start a call; calls are acted on only from the finished `Completion`.

**What it would save.** The model's remaining generation time after its first sentence. For a four-sentence reply written at ordinary speed this is roughly the time to write sentences 2 to 4, on the order of a second or two, plus one synthesis overlapped. Time to first sound becomes speech-to-text + model time to the first sentence + one synthesis. It saves nothing for a one-sentence reply and nothing on rounds that call tools.

**Tests it needs** (fake backend whose chunks are released by the test, as for the satellite tests): a first sentence is on the wire before the backend's last chunk is released; a tool-call delta before the first sentence disables streaming and the words go down the plan path once; words then a call keeps the spoken sentence as the progress phrase and does not repeat it; `too_long`, `last_fail` and limits never stream; the final `speech` plus the streamed sentences equal the model text exactly; cancel mid-stream stops speech and records abandoned usage; a backend error after sentence 1 was spoken ends the turn without a second apology on top of a half answer (the failure account is appended, not substituted); a malformed or oversized stream is cut off at `MAX_STREAMED`; conversation history stores the full final text once.
