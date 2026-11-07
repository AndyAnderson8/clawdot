# Privacy

ClawDot is a device that listens for a wake word, so privacy is a design requirement, not an afterthought. This document describes the **intended** behaviour of the project (pre-alpha; the code does not exist yet) and is a commitment that the implementation is tested against (REQ-SEC-002, REQ-SEC-003, REQ-SEC-007).

## Principles
1. **No audio leaves the device before the wake word.** Wake-word detection runs on-device.
2. **Nothing is sent anywhere you did not configure.** No telemetry, analytics, or crash reporting. ClawDot talks only to the providers and endpoints you set up.
3. **No raw audio is stored or logged**, by default or otherwise.
4. **You can always see and control the microphone state** (mute, listening, streaming).
5. **You choose the providers**, including self-hosted ones.

## Data flow

| Data | Where it goes | When | Stored? |
|---|---|---|---|
| Microphone audio before wake word | Stays on the device (wake-word model only) | Always listening | No |
| Utterance audio after wake word | Your configured **STT provider** | Per turn | Not by ClawDot; the provider's policy applies |
| Transcript text, conversation history, tool schemas, tool results | Your configured **LLM provider** (e.g. OpenRouter, which forwards to the model vendor you select) | Per turn | In memory only, bounded, discarded after the idle timeout; the provider's policy applies |
| Reply text | Your configured **TTS provider** | Per turn | Not by ClawDot |
| Agent-inbox event text (e.g. a coding agent's final message) | Stored on the device; **sent to your LLM provider to be summarised for speech** | When an event is announced | Stored on the device in a bounded inbox; evicted by policy |
| Spoken replies to an agent (as text) | Retrieved by the **agent bridge you configured** over an authenticated stream; the device never calls out to agent hosts | When you reply | Queued briefly on the device until acknowledged, then deleted |
| Requests to HTTP tools / MCP servers you configured (e.g. Home Assistant, Spotify control) | The URL you configured, with parameters the assistant fills in | When the assistant calls the tool | Not by ClawDot |
| Timers, alarms, reminders | Device only | n/a | Yes, on the device |
| API keys and endpoint tokens | Device config only; write-only in the web UI | n/a | Yes, on the device (tokens hashed) |
| Logs | Device only, size-capped, no audio, no secrets | n/a | Rotating |

## What this means in practice
- Whatever your STT, LLM, and TTS providers do with data is governed by **their** terms. Pick them with that in mind. Self-hosted providers keep everything on your network.
- Agent output can be sensitive (code, paths, secrets in logs). Because inbox text reaches your LLM provider for summarisation, use endpoint filters and avoid piping highly sensitive output to ClawDot.
- Physical mute is the strongest control. Satellites declare whether they have one.

## Not covered
Third-party providers, your network's security, and physical access to the device. See the [threat model](docs/security/threat-model.md) and [SECURITY.md](SECURITY.md).

## Changes
This document changes only through a reviewed PR, and a change to any promise above is a breaking change that needs a spec amendment and an ADR.
