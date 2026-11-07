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

Test count at the end: 217 passing across the workspace; clippy `-D warnings` and rustfmt clean.

## Measured budget (scripts/budget.sh, x86_64 musl, 400 hook posts)

| Measure | Result | Limit |
|---|---|---|
| Binary, x86_64 | 2.35 MiB | 8 MiB |
| Binary, armv7 (cross-built) | 1.89 MiB | 8 MiB |
| Idle RSS | 2.8 MiB | 24 MiB |
| Peak RSS under load | 3.8 MiB | 48 MiB target, 64 MiB hard |
| State on disk | 98 KiB | 16 MiB |

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
9. **Landing without a remote is local only.** `scripts/land.sh` says so; the CI-verified path is untested.

## Not done

- Everything audio: satellite, wake word, VAD, STT, TTS (M1). The LLM and inbox work without it via the text console.
- Web UI (M4), music and MCP (M5), Wi-Fi provisioning.
- Releases, changelog entry, tags (human-only).

## Untested or unverified

- The remote path of the landing and release scripts (CI candidate ref, publish) and the GitHub rulesets beyond the partial check.
- The TLS path against a real provider; nothing has called a real LLM.
- armv7 binaries have never been executed; ARM memory and all CPU limits are unmeasured.
- The real `claude` CLI's handling of `-p --resume ... -- <text>` is unverified.
- Codex, Gemini, Hermes, and Home Assistant integration details come from secondary sources.
- `cargo-deny` is not installed locally; only CI runs it.

## For you to decide

- Tighten the budget ratchet from the measured numbers?
- Next milestone: audio (M1) or a real-provider smoke test first? I recommend the smoke test, since it is cheap and
  retires the biggest unknown.
- How to publish: add the remote in a terminal, run the CI-verified landing, then run the release script's publish
  step yourself (agents are blocked from pushing and publishing by design; see the runbook).
