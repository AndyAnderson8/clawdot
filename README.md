# ClawDot

> An open, lightweight, hardware-agnostic voice assistant for smart speakers: wake word, a language model with tools, timers, alarms and music, and a spoken inbox for your own agents.
>
> ClawDot is an independent project and is not affiliated with or endorsed by Anthropic or Amazon. See [ADR-0009](docs/adr/0009-project-name.md).

ClawDot has three goals:

1. **Small.** The whole system must fit a 512 MB smart-speaker-class device, which is the reference budget for every design decision ([ADR-0003](docs/adr/0003-reference-hardware-budget.md)). The core is one program of about 3.5 MB that uses about 3 MB of memory when idle.
2. **Hardware-agnostic.** The core knows no device by name. A small, versioned satellite protocol separates it from the microphone and speaker, and what a device is comes from configuration ([ADR-0005](docs/adr/0005-satellite-core-separation.md), [board support](docs/architecture/board-support.md)).
3. **Bring your own agent.** Anything that can send a webhook or speak MCP (a coding agent, a build server, a script, a home automation hub) can use the speaker as its voice. The speaker says in a sentence or two what happened and lets you answer by voice.

## What it does

- **Answers by voice** through any OpenAI-compatible language-model service, with a daily spending limit that works whether or not the service reports its prices.
- **Uses tools safely.** Built-in tools (timers, alarms, reminders, a stopwatch, music), web tools you describe, and tools from MCP servers. Every call goes through one gate: a risk tier, a spoken read-back of exactly what will run and your "yes" before anything costly, an audit log, and daily caps. Text from outside (web pages, agent messages, tool descriptions) is treated as data, not instructions ([action safety](docs/architecture/action-safety.md)).
- **Speaks for your agents.** Webhooks, a one-line announce API and an MCP server let other programs talk through the speaker and ask you questions ([announce](docs/guides/announce.md), [MCP](docs/architecture/mcp.md)).
- **Remembers what you ask it to**, and follows skills you write or import ([memory and skills](docs/guides/memory-and-skills.md)).
- **Has a web portal** built into the program for setup, settings, tools, activity and logs. Nothing is installed in the browser and nothing is loaded from elsewhere.
- **Speech services are your choice:** an OpenAI-style server (including [one that runs on your own machine](docs/guides/local-speech.md)), ElevenLabs or Deepgram, and an optional realtime voice service ([speech providers](docs/guides/speech-providers.md)).
- **Updates itself** only with releases signed by the project key, and only when you confirm ([updates](docs/guides/updates.md)).

## Status

**Pre-release (version 0.0.0). In daily development; not yet recommended for unattended use.**

| Area | State |
|---|---|
| Core: turns, tools, the gate, inbox, portal, settings, updates | Built, covered by about 850 unit tests, a whole-process test suite and replayed evaluation scenarios, run on every change |
| Runs on a small ARM speaker | Shown on one Echo Dot (2nd generation) running a Linux image: 3.5 MB program, about 3 MB of memory |
| Voice loop end to end | Works with a local speech server; time to the first spoken word is being worked on |
| Realtime voice, ElevenLabs, Deepgram | Built and tested against stand-ins; not yet proven against the real services |
| Speakers that find and share with each other | Discovery only |
| First tagged release | Not yet |

The [roadmap](docs/roadmap.md) has the detail, and [docs/reports](docs/reports/) has dated reviews of what is proven and what is not.

## Quick start

On a computer, with a language-model key in an environment variable:

```
OPENROUTER_API_KEY=your-key clawdot
```

It prints the address of the portal (by default `http://127.0.0.1:3335/`). Open it, choose a password, choose your provider, and type or speak. The [quick start](docs/guides/quick-start.md) walks through it, and [local speech](docs/guides/local-speech.md) adds a voice without an account anywhere.

To build from source you need Rust (the version in `rust-toolchain.toml`); on Windows the scripts build inside WSL:

```
scripts/setup.sh                                 # once after cloning: hooks and identity check
scripts/cargo.sh build --release -p clawdot-core # builds the `clawdot` program
scripts/check.sh                                 # every gate that a change must pass
```

## How it fits together

```
 microphone and speaker             the core (one program)                    the outside
┌──────────────────────┐   ┌───────────────────────────────────────┐   ┌──────────────────────┐
│ satellite            │◄─►│ voice dialog ─► turn ─► gate ─► tools │◄─►│ language model       │
│ (wake word, audio,   │   │ inbox, timers, memory, skills         │   │ speech services      │
│  buttons, lights)    │   │ portal and API                        │◄─►│ webhooks, MCP, agents│
└──────────────────────┘   └───────────────────────────────────────┘   └──────────────────────┘
```

| Crate | What it is |
|---|---|
| `crates/core` | The brain: sessions, the gate, tools, inbox, settings, the portal and its API |
| `crates/protocol` | The versioned wire format between the core and a satellite |
| `crates/satellite` | A reference satellite for a computer or single-board computer |
| `crates/device`, `crates/device-conformance` | The device daemon (audio, buttons, lights from a board file) and its conformance suite |
| `crates/wake` | The wake-word runtime |
| `crates/bridge` | The agent bridge: carries your spoken replies back to a coding agent so it can carry on |
| `bridges/` | Small bridges to chat and notification services |
| `crates/eval`, `crates/e2e` | Evaluation scenarios and end-to-end tests |
| `portal/` | The web portal (TypeScript and Preact), built into one page that the core serves |

Start with the [architecture overview](docs/architecture/overview.md); [docs/README.md](docs/README.md) indexes everything.

## Where to go next

| If you want to | Read |
|---|---|
| Know what is being built and why | [Product requirements](docs/specs/0001-product-requirements.md) |
| See why decisions were made | [Architecture decision records](docs/adr/) |
| Port it to a device | [Porting a satellite](docs/architecture/porting-a-satellite.md), [board support](docs/architecture/board-support.md) |
| Connect an agent or a tool | [Recipes](docs/recipes/README.md) |
| Understand the security model | [Threat model](docs/security/threat-model.md), [SECURITY.md](SECURITY.md), [PRIVACY.md](PRIVACY.md) |
| Contribute | [CONTRIBUTING.md](CONTRIBUTING.md), [development process](docs/process/sdlc.md) |

## License

[Apache License 2.0](LICENSE). Use it however you like, commercially or not, as long as you keep the [`NOTICE`](NOTICE) and licence with any redistribution. See [ADR-0007](docs/adr/0007-license.md).

**Credit, please.** The licence requires keeping the notices. If you build a product or project on ClawDot, we'd also love a visible mention (for example in your README or an "about" page). That's a request, not a licence term.
