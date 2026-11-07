# Future Scope (Parking Lot)

Ideas that are **not** in v1.0. This file keeps good ideas from bloating the initial scope and gives each one a clear condition for promotion. Nothing here is a commitment.

## v1.0 definition (the filter)

> Wake → ask → tools; timers, alarms, and reminders; an agent inbox with spoken replies; a tiny footprint within the Echo Dot Gen 2 budget by default; desktop and Raspberry Pi satellites.

## Triage rubric

An idea is promoted only if it can answer yes to the first question and survive the rest:
1. Does it serve the v1.0 definition, or is it clearly a post-1.0 growth area?
2. Does it fit the budget in the **default** configuration, or can it be strictly opt-in ([spec 0002](specs/0002-resource-budget.md), REQ-NFR-113)?
3. Is it hardware-agnostic (works through the satellite protocol)?
4. Is it maintainable without depending on an unofficial, reverse-engineered, or licence-incompatible third party?
5. Does it have a testable spec?

## Promotion process
Open an issue labelled `type:feature` + `scope:proposal` → write or amend a spec → roadmap change in a PR → only then does it enter a milestone. See [SDLC §5](process/sdlc.md#5-spec-driven-workflow).

## Parked ideas

| Idea | Category | Status / notes | Promote when |
|---|---|---|---|
| **Spotify control** (the maintainer's primary service) via the media-source *control* shape, using its Web API through an HTTP tool or MCP | Integration | Plausible and cheap if API access allows it; requires Premium; verify current API terms for new apps. **Audio plays on the controlled Connect device (phone, another speaker), not on ClawDot's own speaker.** | The media-source interface (M5) is stable and terms are confirmed |
| **Spotify Connect receiver** (ClawDot as a playback device) | Integration | Typically needs reverse-engineered libraries (e.g. librespot); budget and terms-of-service concerns. Research only. | A maintainable, licence-compatible path exists |
| **Voice approval of agent permission prompts** | Agent | High value, high risk. A spoken "yes" could authorise a powerful agent action. Needs its own spec, confirmation design, and threat-model entry. | Inbox and bridge are stable, and a threat model review is done |
| **Long-term memory / notes** | Assistant | Bounded, opt-in tool. Privacy implications. | v1.0 shipped; storage budget measured |
| **Streaming STT as a first-class option** | Voice | Opt-in. Adds provider and test surface. | M1 latency measurements show batch STT misses the target |
| **Barge-in with software AEC** | Voice | Opt-in on satellites that declare AEC. Costs CPU/RAM. | SPIKE-004 shows acceptable cost on the reference class |
| **Custom "ClawDot" wake word training** | Voice | A quality and licence workstream of its own. v1.0 ships an existing, licence-clean pretrained word. | Wake-word eval harness exists |
| **ESP32-S3 satellite** | Hardware | Protocol is designed for it; needs a firmware port. | Protocol is Accepted and conformance suite passes |
| **Wyoming bridge** | Interop | Lets stock Wyoming satellites connect. | ADR-0008 accepted |
| **First-run Wi-Fi provisioning** (captive portal / BLE) | Setup | OS-level and per-platform. Makes the device an appliance. | v1.0 shipped; target platform chosen |
| **Home Assistant: dedicated HACS integration** | Integration | v1.0 covers HA with recipes only (HA → ClawDot via an inbox endpoint; ClawDot → HA via an HTTP tool or MCP). A HACS custom integration (separate repo, runs inside HA) would expose timers, a speak service, and sensors. | v1.0 recipes are stable and there is demand |
| **ClawDot MCP server** (`announce`, `ask_user`) so MCP-capable agents (e.g. Hermes) can speak and ask you questions | Agent | Deferred from v1.0: an agent that can interrupt you and block on your answer is a new attack surface (spoken prompt injection) and needs its own threat model; the MCP spec is still changing. Inbox `kind`/reply types already fit it. | Inbox and bridge are stable; threat model entry written |
| **Agent connect kits** (Claude Code plugin bundling hook + skill; Codex, Gemini CLI, opencode adapters) | Agent | Separate `clawdot-agent-kit` repo. The webhook/SSE transport is harness-agnostic; only the hook config and the resume command differ per harness, and each must be verified from primary docs. | Claude Code reference adapter is stable (M2) |
| **Multiple assistant profiles** (different brains and voices, e.g. Hermes vs a plain model) | Assistant | Selected by voice routing ("ask Hermes ..."), not extra wake words, since each wake word costs CPU/RAM. Needs per-profile provider, persona, voice, and history. | Single-profile persona/voice config is stable |
| **Installable packs** + permission-review UI + optional curated index | Extension | v1.0 uses the pack data format internally for bundled adapters. User-installable packs need the trust model in [extensibility](architecture/extensibility.md). | Adapter engine is stable and there is demand |
| **`/display` page and semantic display events** | Hardware | Lets a satellite with a screen render state, transcript, now-playing, timers, and inbox cards. Protocol reserves `clawdot.display.*`. | A screen satellite exists |
| **Podcasts / audiobooks** (e.g. Audiobookshelf) | Media | Fits the media-source interface with resume positions. | Media interface is stable |
| **Routines** (e.g. morning briefing on an alarm) | Assistant | Triggers + actions; scheduler exists. | v1.0 shipped |
| **Ebooks read aloud** | Media | Cloud TTS for long text is costly; revisit with a cost model. | Cost controls proven |
| **Multi-language** | Voice | English-first for v1.0 (wake word, grammar, prompts). | Wake-word and fast-path grammar localisation plan exists |
| **Multi-room / announcements between satellites** | Audio | Out of scope. | Not planned |
| **Display satellites / status screens** | Hardware | Capability model already allows a display. | Real use case |
| **Echo Dot Gen 2 firmware port** | Hardware | Opportunistic only; no known unlock ([R-04](research/04-echo-dot-gen2.md)). The hardware *budget* is already normative. | Someone gains boot access |
| **Speaker/voice identification** | Assistant | Privacy-heavy, resource-heavy. | Not planned |

## Adding an idea
Add a row with category, honest notes (including risks), and a concrete promotion condition. Prefer "needs research" over a vague yes.
