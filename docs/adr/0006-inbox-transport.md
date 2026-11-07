# ADR-0006: Inbox transport: push-first with device-initiated fallback

- **Status:** Proposed
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

External agents (e.g. Claude Code hooks) must tell the speaker they are done. The original idea was that the speaker polls a webhook every few seconds. Polling works from behind NAT but adds latency and constant traffic. Agents may run on the LAN, on a laptop, or in the cloud.

## Options considered

1. **Device polls a remote URL** every N seconds. Works behind NAT; latency up to N; constant traffic and requires someone to host the mailbox.
2. **Agent pushes to the device** (inbound HTTP endpoint on the core). Instant and simple; works on a LAN/VPN; not reachable across NAT without a tunnel.
3. **Device subscribes to a pub/sub stream** (e.g. SSE against a self-hosted or public ntfy-compatible server). Push-like latency and works behind NAT; needs a relay server.
4. **MQTT.** Robust, but adds client weight and a broker dependency.

## Decision

- **Primary:** inbound authenticated HTTP endpoints on the core (option 2).
- **Fallback:** device-initiated sources, i.e. SSE subscribe (ntfy-compatible) and poll as a last resort (options 3 and 1), all feeding the same inbox.
- Remote reachability is documented as a deployment concern (Tailscale/WireGuard/reverse tunnel recommended). The device is **not** to be exposed directly to the public internet.
- **Format adapters:** each endpoint has a `format` (`claude-code`, `codex`, `gemini`, `generic`) that maps the agent's native hook payload (e.g. Claude Code's native `http` hook) to the internal event model, so setup is a pasted config snippet, not a script ([R-01](../research/01-agent-integration.md)).
- **Reply path via an agent bridge that pulls:** the bridge, running on the developer machine, opens an authenticated **SSE reply stream** to the device and receives replies and acknowledgements (at-least-once, acknowledged, bounded queue with expiry). The bridge resumes the agent (for Claude Code: `claude -p --resume <session_id>`). The device **never initiates connections to agent hosts**, so event bodies cannot cause outbound calls and there is no callback-URL/SSRF surface. Chosen over device-initiated callbacks because it removes that attack surface, works with NAT in the common direction, and keeps outbound traffic limited to providers and admin-configured tools.
- **Reply types** (`ack`, `instruct`, `snooze`, `dismiss`, `details`) and the event `kind` field (`done`, `needs_input`, `question`, `info`) are part of the model so a future ClawDot MCP server (agents calling `announce`/`ask_user`) fits without a protocol change ([future-scope](../future-scope.md)).

## Consequences

**Positive:** low latency in the common LAN case; flexible for NAT scenarios; one inbox model with multiple sources.
**Negative:** more than one ingestion path to test and secure; SSE and poll clients cost some budget (to be measured).
**Follow-ups:** spec REQ-INB-009; measure the SSE client footprint in the budget gate.
