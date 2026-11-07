# ClawDot

> An open, lightweight, hardware-agnostic voice assistant harness for smart speakers. Wake word, LLM tool-calling, timers/alarms/music, and a spoken inbox to BYOA (bring your own agent).
>
> Runs in under 64 MB of RAM (design target: 48 MB).
>
> ClawDot is an independent project and is not affiliated with or endorsed by Anthropic. See [ADR-0009](docs/adr/0009-project-name.md).

ClawDot is a smart-speaker software stack with three goals:

1. **Extremely lightweight.** The whole system must run on an Amazon Echo Dot (Gen 2)-class device (quad Cortex-A53, 512 MB RAM). That device is the *reference budget* for every design decision. See [ADR-0003](docs/adr/0003-reference-hardware-budget.md).
2. **Hardware-agnostic.** A small, versioned satellite protocol separates the brain from the microphone and speaker. A port to a new device only implements the protocol. See [ADR-0005](docs/adr/0005-satellite-core-separation.md).
3. **Bring your own agent (BYOA).** Any agent, script, or service that can send a webhook (Claude Code, Hermes, CI, a cron job, a Home Assistant automation) can use ClawDot as its voice. When it finishes, the speaker plays a tone, tells you in a sentence or two what happened, and lets you reply by voice to send it back to work, or dismiss it.

It also does the basics: wake word, conversational Q&A over any OpenAI-compatible API (such as OpenRouter), tool calling (including MCP), timers, alarms, reminders, and music.

## Status

**Pre-alpha. Specification phase.** No code has been written yet. The project is spec-driven: behaviour is defined in [`docs/specs/`](docs/specs/) before it is implemented.

| Milestone | Goal | State |
|---|---|---|
| M0 Foundations | Specs accepted, repo, CI | In progress |
| M1 Walking skeleton | Wake word, STT, LLM, TTS on a desktop satellite | Not started |

See the [roadmap](docs/roadmap.md).

## Where to start

| If you want to... | Read |
|---|---|
| Understand what we're building and why | [Product requirements](docs/specs/0001-product-requirements.md) |
| Understand how it fits together | [Architecture overview](docs/architecture/overview.md) |
| Contribute | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Understand how work is run | [Development process](docs/process/sdlc.md) |
| See why decisions were made | [Architecture decision records](docs/adr/) |
| Report a vulnerability | [SECURITY.md](SECURITY.md) |

## License

[Apache License 2.0](LICENSE). Use it however you like, commercially or not, as long as you keep the [`NOTICE`](NOTICE) and licence with any redistribution. See [ADR-0007](docs/adr/0007-license.md).

**Credit, please.** The licence requires keeping the notices. If you build a product or project on ClawDot, we'd also love a visible mention (for example in your README or an "about" page). That's a request, not a licence term.
