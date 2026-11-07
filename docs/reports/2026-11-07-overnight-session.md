# Overnight session report, 2026-11-07

Written for the maintainer to review in the morning. Nothing was pushed. All work is on the local `dev` branch,
ahead of the last published state; the `origin` remote was detached for the session and is re-added at the end.

## What exists now

| Area | State |
|---|---|
| `crates/protocol` | Wyoming-compatible framing with hard limits, typed messages, capabilities and versioning, bridge reply wire types. |
| `crates/core` | Agent inbox (adapters, endpoints with hashed tokens, queue, replies, summaries), scheduler (timers, alarms, reminders, DST-correct via bundled tzdb), fast-path command matcher, atomic JSON store, HTTP front end with SSE, text console, **LLM client** (OpenAI-compatible, streaming, tool calls, fallback model, optional rustls), daily request and spend caps, tool loop with enforced confirmation, model-callable timer and inbox tools. |
| `crates/bridge` | Pulls voice replies over SSE and resumes Claude Code (`claude -p --resume`), `--dry-run`, persisted handled ids. |
| `crates/e2e` | Six end-to-end tests of the headline loop over real sockets. |
| Process | Git flow tooling, hooks, agent guard, landing and release scripts, policy tests, budget script. |

Added later in the session (each documented under `docs/architecture/`): a validated JSON config file, an embedded web UI for endpoints and the inbox, declarative HTTP tools, internet radio by voice with a service-agnostic media model, the voice dialog layer (endpointing, mute, half-duplex, follow-up window, speech turn), and OpenAI-compatible speech-to-text and text-to-speech clients, plus tests that run the real binary against fake providers. Test count at the end: 275 passing across the workspace; clippy `-D warnings` and rustfmt clean.

## Measured budget (scripts/budget.sh, x86_64 musl, 400 hook posts, after all features below)

| Measure | Result | Limit |
|---|---|---|
| Binary, x86_64 | 2.45 MiB | 8 MiB |
| Binary, armv7 (cross-built) | 1.97 MiB | 8 MiB |
| Idle RSS | 2.9 MiB | 24 MiB |
| Peak RSS under load | 4.1 MiB | 48 MiB target, 64 MiB hard |
| State on disk | 101 KiB | 16 MiB |

Large headroom. The budget ratchet (REQ-NFR-114) should now be tightened from these numbers; I left the limits
alone because tightening is your call.

## Decisions I made (change any you disagree with)

1. **Bridge talks plain HTTP, std only.** It runs on the same trusted network as the speaker; use a VPN such as
   Tailscale from elsewhere. Keeps the bridge dependency-free.
2. **Hooks answer `{}`** with an `X-ClawDot-Event` header, because Claude Code parses hook output.
3. **Replies are at-least-once.** The bridge persists handled ids, so redelivery never repeats an instruction; a
   command that fails to start is retried.
4. **The session reference must start with a letter or digit**, and spoken text is passed after `--`, so a reply can
   never become a flag.
5. **LLM transport is hand-rolled on hyper**, with rustls (ring, bundled roots) behind a default-on `tls` feature, to
   avoid a large HTTP client dependency. `http://` works without it (local servers).
6. **Confirmation is enforced in code** (`confirmed: true`), not by prompt.
7. **Daily caps default to 500 requests; spend cap off.** Cap reset follows the local calendar day.
8. **Static musl builds link with zig** via small wrappers created by `scripts/setup-wsl.sh`.
. **Config is JSON, flags override it, secrets never live in it.** No TOML parser dependency; unknown keys are errors.
11. **Web UI is one static page plus a small authenticated API.** The administrator token is shown once and stored hashed; agent text is only ever rendered as text, under a strict CSP. Default bind stays on `127.0.0.1`.
12. **HTTP tools confirm by default.** The model fills typed parameters only; URL, headers, and credential are fixed in the config. You can turn confirmation off per tool.
13. **Music: the core never plays audio.** It emits stream effects; the satellite plays and ducks. Control-shape backends (Spotify, MPD, Home Assistant) are expressed as HTTP tools, and those recipes are untested.
14. **Voice: energy-based endpointing first.** No model dependency; a better detector can replace it behind the same interface once measured.
15. **Speech providers: plain `http://` only for localhost and private hosts**, because audio must not cross the internet unencrypted.

## Not done

- Hot reload of the config file (a SHOULD in REQ-CFG-007).
- The satellite itself: microphone and speaker I/O, wake-word detection, and wiring the speech clients and dialog into the binary (M1). The inbox and LLM work today through the text console.
- Web UI pages for provider keys, voice settings, schedules, logs, and "test speak"; mDNS (M4).
- MCP client, resume positions for podcasts, library search (M5); Wi-Fi provisioning.
- The tag itself (human-only). `release/0.1.0` was prepared and landed into `dev` with the generated changelog; Cargo versions are still 0.0.0 because `cargo-set-version` is not installed. To publish, run in a terminal: `scripts/release.sh publish 0.1.0`. It merges `dev` into `main`, tags, and pushes, so it is the one step that needs you. Consider whether v0.1.0 is the right first version with no audio yet.

## Untested or unverified

- The remote path of the landing and release scripts (CI candidate ref, publish) and the GitHub rulesets beyond the partial check.
- The TLS path against a real provider; nothing has called a real LLM.
- armv7 binaries have never been executed; ARM memory and all CPU limits are unmeasured.
- The real `claude` CLI's handling of `-p --resume ... -- <text>` is unverified.
- Codex, Gemini, Hermes, and Home Assistant integration details come from secondary sources.
- `cargo-deny` is not installed locally; only CI runs it.
- The speech clients, HTTP tools, and radio streams have only been run against fake servers.
- Energy-based endpointing has only seen synthetic audio; real rooms (television, fans) are untested.
- The web UI page has never been opened in a browser; the API behind it is tested.

## For you to decide

- Tighten the budget ratchet from the measured numbers?
- Next milestone: audio (M1) or a real-provider smoke test first? I recommend the smoke test, since it is cheap and
  retires the biggest unknown.
- How to publish: add the remote in a terminal, run the CI-verified landing, then run the release script's publish
  step yourself (agents are blocked from pushing and publishing by design; see the runbook).
