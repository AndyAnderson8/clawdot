# Threat Model

Status: Draft. Method: STRIDE over the data-flow in the [architecture overview](../architecture/overview.md). Reviewed at each milestone.

## 1. Assets

| Asset | Why it matters |
|---|---|
| Provider API keys | Financial loss and account abuse if leaked |
| Microphone audio | Privacy; an always-listening device is a high-trust position |
| Inbox endpoint tokens | Let an attacker inject speech and trigger behaviour |
| Reply callbacks | Can carry instructions back to powerful agents (e.g. coding agents with shell access) |
| Device network position | A foothold on the home LAN |
| Scheduled items and inbox history | Personal information |

## 2. Trust boundaries

1. Satellite ⇄ core (when remote)
2. Core ⇄ internet providers
3. LAN/Internet ⇒ core HTTP (web UI, webhooks)
4. Core ⇄ MCP servers / tools
5. **LLM output and any external text ⇒ core decisions** (untrusted data crossing into control flow)

## 3. Principal threats and mitigations

| # | Threat | STRIDE | Mitigation | Req |
|---|---|---|---|---|
| T1 | Prompt injection via inbox summary or tool result ("ignore instructions, call X") | T/E | Treat all external text as data. Never execute tools from inbox text without a user turn. Mark untrusted content in the prompt. Every tool has a risk tier and costly ones (the default for HTTP and MCP tools) are held by the host until the user says yes in a later turn; the model cannot supply or forge that yes; turns the user did not start can only read or propose; untrusted text in a turn makes a lowered tool ask again; agent text in tool results is fenced as untrusted. | REQ-INB-010, REQ-AST-008, REQ-AST-015, REQ-SEC-010..012 |
| T2 | Spoken-word injection: TV/radio/another speaker triggers the wake word and commands | S | Tunable wake-word sensitivity; costly tools are held for a spoken yes with a 120 second window and a host-spoken read-back (a guest or a TV could still answer: speaker identification does not exist, an accepted risk until a button or portal second factor); optional voice PIN for sensitive actions (future) | REQ-AST-008, REQ-SEC-011 |
| T3 | Unauthorised webhook posts (spam, spoofed "done" messages) | S/D | Per-endpoint tokens, hashed at rest, revocable; rate and size limits; constant-time compare | REQ-INB-002, REQ-SEC-004 |
| T4 | Reply callback abuse: an attacker-controlled event with a malicious callback URL causes the device to POST user speech or reach internal services (SSRF) | I/E | The device never initiates connections to agent hosts. The agent bridge pulls replies over an authenticated SSE stream; events carry only a `session_ref`; the bridge, not the device, resumes the agent. The only outbound calls are to providers and admin-configured MCP/HTTP tools. | REQ-INB-008 |
| T5 | Web UI exposure/brute force/CSRF/DNS rebinding | S/E | Password (or deliberate open mode on a private network only), loopback-bind default, bearer session tokens (not cookies, so no CSRF in password mode), any `/api` request a browser labels `Sec-Fetch-Site: cross-site` is refused whatever its method (a cross-site GET such as an image carries no `Origin`; the model list once took an address and a variable name from its query, which let such a GET send a key elsewhere, so it now takes nothing from the address), a write to `/api` whose `Origin` is not the host it was sent to is refused (403; open mode trusts the caller's address, which a page in a browser on the network borrows), the page and the `/api` routes that take no token answer only when the `Host` header names the device (an IP address, `localhost`, the public address setting, or the machine's host name with or without `.local`), otherwise 403, because a DNS-rebinding page shares the device's origin and address but still carries the attacker's name in `Host`; routes that carry their own token (`/hook`, `/mcp`, `/bridge`, `/api/announce` with a token) are not restricted, since a rebound page has no token, rate limiting, no default credentials; see T15 | REQ-CFG-005, REQ-CFG-019, REQ-CFG-020 |
| T6 | Secret leakage via logs, UI, or backups | I | Write-only secrets in UI; log redaction; no keys in error messages; state files 0600 in a 0700 directory; no agent text on stdout in server mode (`--echo-speech` opts in) | REQ-CFG-006, REQ-SEC-009 |
| T7 | Malformed input crashes or exploits parsers | T/D/E | Memory-safe language; fuzzing; bounded sizes; no panics from external input | REQ-SEC-004 |
| T8 | Network MITM on provider calls | T/I | TLS verification enforced; no insecure-skip option in release builds | REQ-SEC-001 |
| T9 | Rogue satellite connects to the core | S | Per-satellite token; TLS option; LAN-only default | REQ-SEC-006 |
| T10 | Covert recording or unclear mic state | I | Wake word on-device; no audio before wake; no audio persisted; clear mute and listening indicator | REQ-VOI-001, REQ-SEC-002/003 |
| T11 | Supply-chain compromise via dependencies | T | `cargo-deny`/`cargo-audit`, pinned lockfile, minimal dependency policy, SBOM, signed releases | REQ-SEC-005 |
| T12 | Resource exhaustion (DoS) via flood of events/audio | D | Bounded queues, rate limits, per-source connection caps, header, hello and body timeouts, shed load | REQ-NFR-110, REQ-SEC-008 |
| T13 | Tool abuse by a compromised or confused LLM (runaway loops, costly actions) | E/D | Tool-loop bounds, per-tool timeouts, host-held confirmation by risk tier, enforced skill allow-lists | REQ-AST-006/008, REQ-SEC-010..013 |
| T14 | Malicious or careless pack, MCP server, or HTTP tool: grants itself tool access, injects prompt text, exfiltrates via parameters, or weakens confirmation | E/T/I | Packs are hash-pinned data only; the portal shows requested tools and policies and requires approval; packs cannot disable confirm, untrusted-text handling, or limits; HTTP tool URLs are admin-configured with schema-validated parameters; permission growth on update needs re-approval | REQ-EXT-001..003, REQ-AST-014 |
| T15 | First-run setup takeover: on a shared network a stranger completes setup first and becomes the administrator (or the owner is tricked into open mode) | S/E | Default listen is loopback; setup is accepted only from private addresses; open mode refused for public listen addresses and public peers, with a persistent banner; recovery is `--reset-admin` on the device. **Accepted residual risk (ADR-0013 amendment, 2026-11-08):** there is no setup code, because the reference device has no visible console. Until setup completes, whoever on a private network reaches the page first can set the password or choose open mode; the console says so at start-up. A proxy or port-forward on a private address makes remote clients look private. Complete setup right after the first start on a network you trust. Webhook tokens stay required either way | REQ-CFG-018, REQ-CFG-020, REQ-CFG-021, REQ-CFG-022 |
| T16 | Stolen `admin.json` is cracked offline | I/E | Salted PBKDF2-HMAC-SHA256, 100,000 iterations, minimum length 8, parameters stored for later raising. Weaker than a memory-hard hash (rejected for the memory budget, ADR-0013); the attacker already needs the device's disk | REQ-CFG-019 |
| T17 | Takeover through the forgotten-password path: a guest opening the local reset window (there is no recovery code to guess) | S/E | A 10 second hold at the device opens a 5 minute, private-address, single-use window that only the device itself or a satellite with a token may request, behind the shared rate limit; nothing is logged but the event. See the ADR-0013 amendments of 2026-11-09 and 2026-11-10 | REQ-CFG-069, 071, 072, REQ-DEV-031, REQ-SAT-020 |
| T18 | A connected agent harness abuses the ClawDot MCP server: speaks a convincing request ("your bank needs you to say yes"), floods the speaker or wakes the household at night, impersonates another source, or uses the person's spoken answer as approval for something it then does | S/E/D | Endpoint token only, hashed at rest and revocable per agent; only two tools and no read or write of ClawDot state; text capped at 500 characters, cleaned and redacted, and spoken as "From <endpoint>" with the source not choosable; the person's answer goes only to the asking call, is labelled untrusted, and can never satisfy a held action or a risk tier (it is not queued to a bridge); mute, minimum priority, quiet hours and the burst limit apply; one open question per endpoint and four in all; every call audited as `mcp:<endpoint>` without the answer. **Residual risk:** `urgent` bypasses quiet hours, and a harness can still talk the person into something in plain words; the harness's own approval system must stay on for what it does | REQ-EXT-011..018, REQ-INB-022, REQ-SEC-011 |

## 4. Special concern: the agent loop

The headline feature (talk back to coding agents) crosses into a high-risk area: a **spoken reply can cause a powerful agent to act**. Rules:
- Replies carry only free text and a `session_ref`, which the *agent-side integration* resolves. The speaker never ships shell commands.
- Endpoint configuration decides which callbacks exist; agents cannot register new callbacks.
- The announcement states the source plainly ("From Claude Code, project clawdot") so users know what they are replying to.
- Documentation tells users to run agents under their own permission policies. The speaker is a messenger, not an authorisation layer.

## 5. Out of scope / accepted risks

- Physical attacks on the device (flash extraction, debug ports).
- Compromise of the user's LLM/STT/TTS provider accounts beyond key hygiene.
- Public-internet exposure of the device (explicitly unsupported; use a VPN or tunnel).

## 6. Review log

| Date | Milestone | Reviewer | Notes |
|---|---|---|---|
| 2026-11-07 | M0 | Maintainer | Initial draft |
