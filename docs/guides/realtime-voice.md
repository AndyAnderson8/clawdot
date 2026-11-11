# Realtime voice (optional)

ClawDot normally listens, thinks and speaks in three steps: speech to text, a language model, then text to speech. A **realtime** provider does all three over one streaming connection, which can feel faster and more natural. It is optional, it is off unless you choose it, and the three-step pipeline stays the default and the fallback. The design is in [ADR-0020](../adr/0020-realtime-voice-provider.md); the requirements are REQ-RTV-001 to REQ-RTV-007.

**Status: built and tested against a fake server only. It has not been run against the real service**, and it is not yet wired into the satellite loop or the portal's Voice page (see "What is and is not done").

## Where it runs

On a PC or a Raspberry Pi, or through a small relay. It is not meant for the Echo Dot's own memory budget ([spec 0002](../specs/0002-resource-budget.md)): it holds a live encrypted connection and streams 24 kHz audio both ways. A Dot can still use it as a satellite of a ClawDot core running elsewhere.

## Turning it on

It is a cargo feature, off by default, so the normal build does not change and adds no crate:

```
cargo build --release -p clawdot-core --features realtime
```

The feature uses the TLS support already in the build (`tls`). At run time you give it a service address, a model, and an API key. Keep the key in the environment or the secrets store, never in a file you share:

| Setting | Default | Notes |
|---|---|---|
| `url` | none | `wss://api.openai.com/v1/realtime` for OpenAI. Plain `ws://` is accepted only for this machine or the home network |
| `model` | none | for example `gpt-realtime` (check the provider's current names) |
| `voice` | `alloy` | the provider's voice name |
| `transcription_model` | `gpt-4o-mini-transcribe` | transcribes what you say so ClawDot can read your yes or no; leave empty and held actions cannot be confirmed by voice |
| `usd_per_min_in` / `usd_per_min_out` | 0.03 / 0.10 | what you pay per minute of audio sent / received; see below |
| `max_session` | 5 minutes | a session ends at this length |

The code entry points are `clawdot_core::realtime::{connect, run, OpenAi}` (`crates/core/src/realtime.rs`).

## What it costs

OpenAI lists `gpt-realtime` audio at $32 per million input tokens and $64 per million output tokens ([pricing page](https://developers.openai.com/api/docs/pricing), read 2026-11-11). How many tokens a minute of audio is, is the provider's business; the default rates above are a deliberate over-estimate so that the cap errs on the safe side. **Set your own from the provider's page.**

ClawDot does not trust a provider's usage report. It counts the seconds of audio it sends and receives itself, multiplies by your rates, and adds that to the same daily counters as the language model, so the **daily spend cap** (REQ-AST-011) stops a realtime session too: a session will not start when the cap is already reached, and a running one ends with a spoken explanation when it is reached. While a session is open the microphone streams continuously, so you pay for input audio for the whole session, not only while you talk; the five-minute length limit bounds that. Each batch of tool calls also shows as two requests in the day's request count; that count has no limit.

## What it can and cannot do

- **Tools stay in ClawDot.** The model sees the tools your host offers and can ask for them, but ClawDot runs them through the same code as any other turn. Read-only and reversible tools run; a costly tool is held, ClawDot asks you out loud, and it runs only when your own next words, as transcribed, are a plain yes. Nothing the model says can confirm for it. Destructive tools are not offered unless you enabled them. The audit log, daily action caps, dry-run and turn limits all apply. Text a tool returns from outside (a web page, an agent event) is handed to the model labelled as untrusted data.
- **Nothing is stored.** Audio is forwarded and dropped. Your transcript is kept in memory for the session only, to read your yes or no, and is gone when the session ends. Nothing is written to disk or logs.
- **Audio leaves the device.** While a session is open, what the microphone hears goes to the provider. Mute the microphone or end the session if that is not acceptable. This is the biggest difference from the default pipeline.
- **No memory between sessions** beyond what ClawDot itself keeps for a conversation, and no wake word or follow-up window logic of its own: it is a session you start and end.

## Limits you should know about

- **Not run against the real service.** The protocol (URL, `Authorization` header, `session.update`, `input_audio_buffer.append`, `response.output_audio.delta`, `response.done` function calls, `conversation.item.create` with `function_call_output`, `response.create`) follows OpenAI's documentation, but the fake server I tested with is my reading of it. Not verified: the exact name and place of the input transcription setting (`audio.input.transcription`), whether the transcript event arrives for every utterance, and how the service behaves under `server_vad` when ClawDot's reply instructions arrive at the same time as an automatic reply.
- **The question is spoken by the model.** For a held action ClawDot gives the model the exact words and tells it to say them; the model could word it differently. What runs is still exactly the stored action, never the model's paraphrase, but a changed read-back could mislead you. Speaking ClawDot's own words with its text to speech is future work.
- **A race fails safe.** Your transcript arrives after the model starts replying. If the model calls a function before the transcript of your "yes" arrives, the held action is dropped as an unrelated request and you have to ask again.
- **Audio is not echo-cancelled.** Use headphones or a speaker setup where the microphone does not hear the speaker, or the model may hear itself.
- **No certificate check of `Sec-WebSocket-Accept`.** The built-in WebSocket client skips it (TLS already authenticates the server).

## What is and is not done

Done (branch `feature/realtime-voice`): the protocol dialect, the WebSocket client, the session engine, the cost counting, the tests. Not done: starting a session from the wake word or the console, reading these settings from the configuration file and the portal's Voice page, playing the audio through the satellite's speaker (the engine takes and gives 24 kHz mono PCM; `realtime::resample` converts), and the real-service run that would turn "follows the documentation" into "works".
