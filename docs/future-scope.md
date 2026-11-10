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
| **Board support layer and Echo Dot Gen 2 target** | Hardware | A `Board` trait (LEDs, buttons, mic-mute, Wi-Fi, clock) with a null and desktop implementation, a `dot2` build profile and per-device JSON. One repo, not a fork. Findings are in [R-07](research/07-echo-dot-gen2-hardware.md); the design is [ADR-0012](adr/0012-board-support-layer.md) (Proposed) and spec 0001 §4.10. Unachievable or unproven today: electrical mute proof, hardware AEC, autostart, safe audio takeover. | ADR-0012 accepted, and the R-07 unknowns for buttons, audio takeover and autostart are resolved on the device |
| **ESPHome native-API voice-satellite client** (roadmap B8, a compatibility mode; specified in [spec 0004](specs/0004-esphome-satellite-client.md) and built behind the off-by-default features `esphome-satellite` and `esphome-noise`, see [architecture/esphome-satellite.md](architecture/esphome-satellite.md)) | Interop | A core feature `esphome-satellite` behind the satellite interface so that a Dot running a base image, or any ESPHome voice satellite, uses ClawDot as its brain without Home Assistant. Hand-written frame layer and minimal protobuf, optional Noise encryption on `ring`, about 1,100 to 1,300 lines (estimate), tested against a fake server first. Design in [R-08](research/08-existing-echo-dot-projects.md) sections 4.2 and 4.3. Needs a spec amendment (a REQ-SAT appendix, and REQ-SAT-014 wording that Wyoming is for stock Wyoming satellites and ESPHome for ESPHome ones) before code. | Maintainer accepts R-08's recommendation and the B0 trial shows the base serves a non-Home-Assistant client |
| **Multi-user admin accounts** | Setup | Not planned: one household, one owner, one admin password; agents use scoped webhook tokens. | A real multi-owner use case appears |
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
| **Zigbee for devices with a radio** | Hardware | Pair and control lights, plugs and sensors locally, as optional tools, without a hub or Home Assistant. | A board or dongle with a Zigbee radio, and a real use case |
| **Echo Dot Gen 2 firmware port** | Hardware | Opportunistic only; no known unlock for retail units ([R-04](research/04-echo-dot-gen2.md)). An adb-root unit already runs ClawDot as an ordinary process ([R-07](research/07-echo-dot-gen2-hardware.md)), which the board layer builds on. The hardware *budget* is already normative. | Someone gains boot access |
| **Speaker/voice identification** | Assistant | Privacy-heavy, resource-heavy. | Not planned |
| **Quiet hours and DND for agent announcements**, with priority bypass and a held queue | Agent | No compared project does this; their DND silences HA announcements only. Read the base's DND entity where present. See [R-11](research/11-open-source-echo-feature-landscape.md) 4.2. | Inbox priority mapping is stable |
| **Voice-turn history** (RAM-only, off by default, clearable) with inbox traceability | Privacy | TECHO5 and LibreEcho keep the last 10 turns in RAM; match that default and link each turn to its inbox item. | Portal history view is specified |
| **Log viewer and diagnostics bundle** with secret and transcript redaction | Operations | Every compared project has one; ours must redact webhook secrets, tokens and spoken text by default. | Portal logging endpoint exists |
| **Settings export/import** for ClawDot config, secrets excluded | Operations | Model: TECHO5 never exports keys or privacy switches. Per-group saving already exists in the portal. | Config schema is stable |
| **Signed OTA with rollback for the ClawDot binary** (not the OS) | Operations | The base updates the OS; the binary needs its own atomic, signed, rollback-capable update on Dot, Pi and PC. Needs an ADR for key handling. | Release pipeline produces signed artifacts |
| **Assistant-slot routing** (second wake word or spoken prefix to ClawDot; the first stays on the base's own assistant) | Assistant | Keeps the base useful as a fallback and avoids a hard dependency. Depends on the B0 finding that a non-HA client can own a slot. Avoid TECHO5's shared-conversation trap (its issue 328). | B0 trial shows the base serves a non-HA client |
| **Wake-word near-miss capture** (opt-in, local only) | Voice | For the PC/Pi wake engine tuning; on the Dot read the base's score events instead. TECHO5 and EchoMuse have it. | Wake-word eval harness exists |
| **HTTPS option** for the portal and webhook receiver (off by default) | Security | Webhook secrets cross the LAN in clear text otherwise; `rustls` is already in the lockfile. Self-signed or local CA. | Threat model entry accepted |
| **Where-to-speak choice for inbox events** (nearest or last-used device) | Audio | Differs from the base's nearest-device-answers (that is for questions). Only matters with two or more satellites. | A second satellite is in use |
| **Realtime speech-to-speech provider** (user key, opt-in, minute cap) | Voice | TECHO5 shows design and costs (daily minute cap, no memory between turns). Bypasses our STT/LLM/TTS split; reuse its rule that nothing changes state after reading outside content. | Provider interface and cost caps are stable |
| **ChatGPT-subscription sign-in as an LLM provider** | Providers | LibreEcho does it through an undocumented backend; terms and stability unknown. Watch only. | OpenAI documents a third-party route |
| **Base-control tool** (volume, mute state, radio, ring effect) exposed to the agent via MCP/HTTP tool | Agent | Entities the base already exposes over ESPHome become one tool. | B2 ESPHome client works |

## Adding an idea
Add a row with category, honest notes (including risks), and a concrete promotion condition. Prefer "needs research" over a vague yes.
