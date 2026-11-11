# ADR-0020: Realtime voice as an optional provider

- **Status:** Proposed
- **Date:** 2026-11-11
- **Deciders:** Maintainer

## Context

ClawDot listens, thinks and speaks in three separate steps (speech to text, language model, text to speech). Each hop adds delay. Some providers offer a single streaming connection that takes microphone audio and returns spoken audio ("realtime" or speech-to-speech), and can call functions in the middle of a reply. Research note 02 first left this out of scope; the roadmap (item 14) now asks for it as an option.

Forces:

- The safety design is ClawDot's own: risk tiers, host-held confirmation bound to the exact action, the audit log, the untrusted-text rule (REQ-SEC-010 to 013, REQ-AST-008). A provider that runs its own tools would bypass all of it.
- The spend cap (REQ-AST-011) is derived from provider usage. Realtime is billed by audio time, so a session could spend freely if it were not counted.
- A streaming WebSocket with 24 kHz audio in both directions does not fit the Echo Dot Gen 2 budget (spec 0002). The default build must not change.
- Audio and transcripts are the most sensitive data the product handles (REQ-SEC-007).

## Decision

1. **A separate, optional provider, behind the cargo feature `realtime`, off by default.** The feature depends on `tls` and adds no crate. The default build is unchanged. It is also off at run time unless the administrator chooses it; the cascaded pipeline stays the default and the fallback.
2. **The provider is a protocol dialect, not a second assistant.** A small `Dialect` trait turns ClawDot's needs (session setup with tools, append microphone audio, return a function result, ask for a reply) into one provider's JSON events and turns the provider's events into a few neutral ones (audio out, user transcript, function calls, error). The first dialect is the OpenAI Realtime WebSocket protocol (PCM16, 24 kHz). Others (xAI, Gemini) add a dialect, not a new engine.
3. **Tool calls always go through ClawDot's tool host.** The realtime model only proposes calls. When a response ends with function calls, the engine runs them as one ordinary turn through `session::run_turn` with a scripted backend that yields exactly those calls. Risk tiers, held actions, the taint rule, skill allow-lists, dry-run, daily caps, the audit log, turn limits and per-call timeouts therefore apply with no second implementation. A held action is asked out loud and released only by the user's next transcribed words, read by the host (`is_yes`), never by the model.
4. **Audio time counts toward the daily spend cap.** The engine counts the seconds of audio it sends and receives itself (it does not trust a provider's report), multiplies by configured per-minute rates, adds the cost to the usage tracker (`UsageTracker::add_cost`) as it goes, and ends the session, with a spoken explanation, when the cap is reached. A session also has a hard length limit.
5. **No audio or transcript is stored or logged.** Audio chunks are forwarded and dropped. The user's transcript is held in memory only to run the host's yes or no check and to start tool turns, and is dropped when the session ends. Errors shown to the user are plain sentences that never contain the key.
6. **Where it runs:** on a PC or a Raspberry Pi, or through a small relay, never inside the Dot's memory budget. A Dot can still use it by being a satellite of a core running elsewhere.
7. **No new dependency: a minimal WebSocket client** (`crates/core/src/realtime/ws.rs`, a few hundred lines), using the TLS stack already built for the language-model client. A client crate such as `tokio-tungstenite` pulls in `tungstenite`, `sha1`, `base64`, `url`, `http`, `httparse` and more, for a feature that uses a fraction of them. The cost of writing our own is that we own the framing bugs, so it is small, bounded (messages are capped) and tested against a fake server. It does not check `Sec-WebSocket-Accept` (that needs SHA-1, and TLS already authenticates the server).

## Consequences

**Positive:**
- Lower delay and more natural turns where the user wants them, with nothing different for anyone else.
- The safety model is not forked: one tier and confirmation implementation.
- Spend is bounded by the same cap as everything else.

**Negative and limits:**
- Audio leaves the device while the user speaks, to a third party, for the whole session (the microphone streams continuously while a session is open). The guide says so.
- The host's question for a held action is spoken by the realtime model from an instruction ("say exactly this"). The model could word it differently. What runs is still exactly the stored action, but a paraphrased read-back could mislead. Speaking the host's text with ClawDot's own text to speech is future work.
- The user's transcript arrives after the model starts replying. If the model calls a function before the transcript of the user's "yes" arrives, a held action is dropped as an unrelated request and the user has to ask again. This fails safe.
- No stop-word handling during a reply beyond the provider's own interruption.
- At the time of this ADR it has not been run against the real service; only against a fake server that follows the documented events. The protocol assumptions that are not verified are listed in the guide.

**Follow-ups:**
- Wire the session into the satellite loop and the Voice page (a separate branch).
- A second dialect once the first has been run for real.
- Speak held-action questions with ClawDot's own voice.
