# Realtime voice (optional)

ClawDot normally listens, thinks and speaks in three steps: speech to text, a language model, then text to speech. A **realtime** provider does all three over one streaming connection, which can feel faster and more natural. It is optional, it is off unless you choose it, and the three-step pipeline stays the default and the fallback. The design is in [ADR-0020](../adr/0020-realtime-voice-provider.md); the requirements are REQ-RTV-001 to REQ-RTV-007.

**Status: built, wired into the speaker loop and tested against a fake server only. It has not been run against the real service.** The portal's Voice page does not show it yet (the settings and the status are there for it). See "Still to check against the real service" for what a first real run must confirm.

## Where it runs

On a PC or a Raspberry Pi, or through a small relay. It is not meant for the Echo Dot's own memory budget ([spec 0002](../specs/0002-resource-budget.md)): it holds a live encrypted connection and streams 24 kHz audio both ways. A Dot can still use it as a satellite of a ClawDot core running elsewhere.

## Turning it on

It is a cargo feature, off by default, so the normal build does not change and adds no crate:

```
cargo build --release -p clawdot-core --features realtime
```

The feature uses the TLS support already in the build (`tls`). At run time you give it a service address, a model, and an API key. The key is never a value in the configuration file or in a setting: put it in the environment variable named by `key_env` (default `OPENAI_API_KEY`) where ClawDot runs, or paste it into the portal, which keeps it in the private secrets file like the language-model key. The settings page and the API only ever show whether realtime is configured, the provider name and a masked hint of a pasted key.

Configure it in the `voice.realtime` section of the configuration file, or with the portal settings (a saved setting beats the file; every one applies without a restart):

| File key (`voice.realtime.`) | Portal setting | Default | Notes |
|---|---|---|---|
| `enabled` | `realtime_enabled` | off | turning it on is refused when the build lacks the feature, the address or model is missing, or no key is available |
| `provider` | none | `openai` | the only dialect there is |
| `url` | `realtime_url` | none | `wss://api.openai.com/v1/realtime` for OpenAI. Plain `ws://` is accepted only for this machine or the home network |
| `model` | `realtime_model` | none | for example `gpt-realtime` (check the provider's current names); with `url`, both or neither |
| `voice` | `realtime_voice` | `alloy` | the provider's voice name |
| `key_env` | `realtime_key_env` | `OPENAI_API_KEY` | the NAME of the environment variable that holds the key |
| `transcription_model` | `realtime_transcription_model` | `gpt-4o-mini-transcribe` | transcribes what you say so ClawDot can read your yes or no; empty (in the file) means held actions cannot be confirmed by voice |
| `usd_per_min_in` / `usd_per_min_out` | `realtime_usd_per_min_in` / `realtime_usd_per_min_out` | 0.03 / 0.10 | what you pay per minute of audio sent / received, 0 to 100; see below |
| `max_session_secs` | `realtime_max_session_secs` | 300 | a session ends at this length (10 to 3600) |

```json
{ "voice": { "realtime": { "enabled": true, "url": "wss://api.openai.com/v1/realtime", "model": "gpt-realtime", "key_env": "OPENAI_API_KEY" } } }
```

`GET /api/settings` carries `realtime: {configured, built, provider, key_hint}` and `GET /api/status` carries `realtime: {configured, active, last_error}`.

## What happens at the speaker

When realtime is configured and a speaker is idle and unmuted, the wake word (or the action button) starts one realtime session in place of listen, think and speak:

- The speaker's microphone audio (16 kHz) is converted to 24 kHz and streamed to the session. The model's audio comes back through the speaker's normal audio path (converted to 16 kHz, start, chunks, stop), with music paused for the conversation and resumed when it ends.
- Barge-in: when the provider says you began speaking, or you say the wake word, playback stops at once. The action button ends the session; so does muting the speaker, and so does about 10 seconds with nobody speaking, so the microphone does not stream (and bill) for nothing.
- Only one session runs at a time. Tool effects such as volume or starting a stream are applied when the session ends, not in the middle.
- The core's main loop runs the session, so while one is open (at most `max_session_secs`) the core does not service the portal's changes, timers or other speakers; the status is still updated. This is a limit of the first version, not a design goal.
- The daily spend cap is checked **before each session opens**: a day already over its cap opens no connection and the speaker says the spending-limit message.
- **Any failure falls back to the normal path.** If the service cannot be reached, rejects the key, is out of quota, drops the connection or sends something unusable, the speaker says the plain message in its normal voice, the status shows it in `last_error`, and for the next 60 seconds the wake word uses listen, think and speak without trying realtime again. That request is not retried for you: the audio had already gone to the session, so ask again.

The code entry points are `clawdot_core::realtime::{connect, run, serve::serve, OpenAi}` (`crates/core/src/realtime.rs`, `crates/core/src/realtime/serve.rs`), the speaker side is in `crates/core/src/satellite.rs`, and the settings and status handle is `crates/core/src/rtvoice.rs`.

## What it costs

OpenAI lists `gpt-realtime` audio at $32 per million input tokens and $64 per million output tokens ([pricing page](https://developers.openai.com/api/docs/pricing), read 2026-11-11). How many tokens a minute of audio is, is the provider's business; the default rates above are a deliberate over-estimate so that the cap errs on the safe side. **Set your own from the provider's page.**

ClawDot does not trust a provider's usage report. It counts the seconds of audio it sends and receives itself, multiplies by your rates, and adds that to the same daily counters as the language model, so the **daily spend cap** (REQ-AST-011) stops a realtime session too: a session will not start when the cap is already reached, and a running one ends with a spoken explanation when it is reached. While a session is open the microphone streams continuously, so you pay for input audio for the whole session, not only while you talk; the five-minute length limit bounds that. Each batch of tool calls also shows as two requests in the day's request count; that count has no limit.

## What it can and cannot do

- **Tools stay in ClawDot.** The model sees the tools your host offers and can ask for them, but ClawDot runs them through the same code as any other turn. Read-only and reversible tools run; a costly tool is held, ClawDot asks you out loud, and it runs only when your own next words, as transcribed, are a plain yes. Nothing the model says can confirm for it. Destructive tools are not offered unless you enabled them. The audit log, daily action caps, dry-run and turn limits all apply. Text a tool returns from outside (a web page, an agent event) is handed to the model labelled as untrusted data.
- **Nothing is stored.** Audio is forwarded and dropped. Your transcript is kept in memory for the session only, to read your yes or no, and is gone when the session ends. Nothing is written to disk or logs.
- **Audio leaves the device.** While a session is open, what the microphone hears goes to the provider. Mute the microphone or end the session if that is not acceptable. This is the biggest difference from the default pipeline.
- **No memory between sessions** beyond what ClawDot itself keeps for a conversation, and no follow-up window: a session is one continuous conversation that the wake word starts and silence, the action button or muting ends.

## Limits you should know about

- **Not run against the real service.** The protocol (URL, `Authorization` header, `session.update`, `input_audio_buffer.append`, `response.output_audio.delta`, `response.done` function calls, `conversation.item.create` with `function_call_output`, `response.create`) follows OpenAI's documentation, but the fake server I tested with is my reading of it. Not verified: the exact name and place of the input transcription setting (`audio.input.transcription`), whether the transcript event arrives for every utterance, and how the service behaves under `server_vad` when ClawDot's reply instructions arrive at the same time as an automatic reply.
- **The question is spoken by the model.** For a held action ClawDot gives the model the exact words and tells it to say them; the model could word it differently. What runs is still exactly the stored action, never the model's paraphrase, but a changed read-back could mislead you. Speaking ClawDot's own words with its text to speech is future work.
- **A race fails safe.** Your transcript arrives after the model starts replying. If the model calls a function before the transcript of your "yes" arrives, the held action is dropped as an unrelated request and you have to ask again.
- **Audio is not echo-cancelled.** Use headphones or a speaker setup where the microphone does not hear the speaker, or the model may hear itself.
- **No certificate check of `Sec-WebSocket-Accept`.** The built-in WebSocket client skips it (TLS already authenticates the server).

## What is and is not done

Done: the protocol dialect, the WebSocket client, the session engine, the cost counting, the configuration and settings, the status, starting a session from the wake word, playing its audio through the speaker's path, barge-in, the fallback with a spoken message, and tests that run a whole turn through the speaker connection against a fake provider on loopback (including failure, an unreachable service, the cap and the exclusive session). Not done: the portal's Voice page for these settings, a field to paste the key in the portal (the secret name `realtime_key` exists; only the environment variable can be used until a page sets it), starting a session from the console, and the real-service run.

## Still to check against the real service

Everything below passes against the fake server because the fake was written from the same reading of the documentation as the code. A first run against the real service, on a speaker, must confirm each of these, and any that differ is a bug to fix before this is called working:

1. **Session setup.** That `session.update` with `type: "realtime"`, `audio.input.format {type: "audio/pcm", rate: 24000}`, `audio.input.transcription`, `audio.output.format`/`voice`, `tools` and `tool_choice` is accepted, and that the key goes in the `Authorization: Bearer` header on the upgrade as sent. The model query parameter (`?model=`) and the URL.
2. **Events by name.** That the events are named as parsed: `response.output_audio.delta`, `input_audio_buffer.speech_started`, `conversation.item.input_audio_transcription.completed`, `response.created`, `response.done` (function calls inside `response.output`), `error`. Whether a transcript arrives for every utterance, and how late.
3. **Automatic replies and ClawDot's own.** With `server_vad` the provider starts a reply by itself when you stop speaking; ClawDot also sends `response.create` after tool results and for held-action questions. Whether the two collide (an `conversation_already_has_active_response` error is treated as harmless) or produce a double answer.
4. **Barge-in.** Whether the provider cancels its response when you speak (and stops sending audio), or whether stale audio deltas still arrive after the flush and are played. If they do, the speaker needs to ignore audio until the next response starts.
5. **Spoken confirmation.** That a held action's question is spoken as given, that your "yes" is transcribed in time to be read before the model next calls a function, and what the race documented below looks like in practice.
6. **Cost.** The default rates are an over-estimate. Compare ClawDot's counted spend for a measured session with the provider's billing and set your own rates. Check that continuous microphone streaming while the session is quiet is billed as input and that the 10-second quiet timeout is short enough.
7. **Audio on the speaker.** That the streamed reply (one start, chunks as they arrive, a stop after 300 ms without audio) plays without gaps or clicks on a real satellite, that a stop mid-reply is followed or not by a playback-done message (the speaker does not wait for one), and that a reply of several segments is handled when a stop and a new start arrive while the first is still playing.
8. **Echo.** Whether the model hears itself through a real speaker and answers its own voice, since nothing cancels echo.
9. **Limits.** What the provider does at its own maximum session length, and that a connection closed by the provider mid-reply is reported as such. Whether a long-lived WebSocket survives the speaker's network.
10. **The busy core.** Whether holding the main loop for the length of a session is acceptable in daily use (alarms and portal changes wait until it ends), or the session must move to its own task.
