# Privacy

ClawDot listens for a wake word, so privacy is a design requirement. This document says what the code does today (pre-release, version 0.0.0). The promises in it are tested (REQ-SEC-002, REQ-SEC-003, REQ-SEC-007), and a change to one is a breaking change that needs a spec amendment and an ADR.

## Principles

1. **No audio is sent anywhere before the wake word or a button press.** The core asks a satellite for audio only while the dialog is listening, and the wake word runs on the device side.
2. **Nothing is sent anywhere you did not configure.** No telemetry, analytics or crash reporting. The one request ClawDot makes on its own is the update check (below), and it can be turned off.
3. **No raw audio is stored or logged.**
4. **You can see and control the microphone state** (mute, listening, thinking, speaking).
5. **You choose the providers**, including ones that run on your own network.

## Data flow

| Data | Where it goes | When | Stored by ClawDot? |
|---|---|---|---|
| Microphone audio before the wake word | Stays on the device | Always | No |
| Utterance audio after the wake word | Your speech-to-text service. With the optional realtime voice, the audio streams to that provider for the session | Each turn | No; the service's own policy applies |
| Transcript, conversation history, tool descriptions, tool results, and any notes you asked it to keep that fit the request | Your language-model service | Each turn | History in memory only, bounded, dropped after 10 minutes idle |
| Reply text | Your text-to-speech service | Each turn | No |
| Event text from agents (for example a coding agent's last message) | Stored in the device's inbox; sent to your language model to be summarised for speech | When announced | Yes, bounded inbox |
| Your spoken replies to an agent, as text | Read by the agent bridge you set up, over an authenticated stream; the device does not call out to agent hosts | When you reply | Queued until acknowledged, or for up to 24 hours |
| Requests to HTTP tools and MCP servers you configured | The address you configured, with the parameters the assistant filled in | When the assistant calls the tool | No |
| Notes ("remember that ...") | Device only; relevant ones are added to the model's prompt | Only if you turn notes on | Yes, in `memory.json`, at most 500 notes |
| Audit log of actions (tool, tier, redacted arguments, outcome, up to 2,000 characters of a result) | Device only; shown in the portal | Each action | Yes, in `audit.jsonl`, about 512 KB at most, rotating |
| Timers, alarms, reminders, usage counters, settings | Device only | n/a | Yes |
| Provider keys and endpoint tokens | Device only. A key is read from an environment variable, or pasted into the portal and kept in `secrets.json`. Tokens are stored hashed. Keys are masked in the portal and left out of settings exports and diagnostics | n/a | Yes; the key file is private to the service user, not encrypted |
| Recent warnings and errors | Device only, in memory, redacted, lost at restart | n/a | No |
| Update check | A request for the latest release page on GitHub (on by default; the `updates.check` setting turns it off) about once a day. It reveals the device's public address to that host and nothing else. Installing needs your confirmation | Daily | The result only |
| Hive discovery (optional build and flag) | Announces an id label on the local network by mDNS; the device name only if you ask | Only with `--hive-discovery` | The device identity key |

## What this means in practice

- What your speech and language-model services do with data is governed by their terms. Self-hosted services keep everything on your network.
- Agent output can be sensitive. State files are readable only by the service user, and in server mode (`--no-console`) agent text is not written to standard output unless you pass `--echo-speech`. Because event text reaches your language model for summarising, use endpoint filters and do not pipe highly sensitive output to ClawDot.
- The portal and the satellite port use plain HTTP and TCP. Use a trusted network or a VPN.
- A hardware mute is the strongest control. Satellites declare whether they have one.

## Not covered

Third-party services, your network's security, and physical access to the device. See the [threat model](docs/security/threat-model.md) and [SECURITY.md](SECURITY.md).
