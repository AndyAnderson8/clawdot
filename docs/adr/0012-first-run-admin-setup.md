# ADR-0012: First-run administrator setup instead of a printed token

- **Status:** Proposed
- **Date:** 2026-11-08
- **Deciders:** Maintainer

## Context

The first web UI printed an administrator token once on the console and asked for it on a "Sign in" page. On a headless speaker there is often no console to read, a lost token needs shell access to replace, and the token is awkward to type on a phone. The maintainer wants a normal first-run experience: choose a password, or deliberately leave the UI open on a trusted network. The device must stay within the Echo Dot Gen 2 budget ([ADR-0003](0003-reference-hardware-budget.md)), and the UI must not become a way onto the LAN for strangers.

## Decision

1. **One administrator, one password. No accounts.** Multi-user admin stays parked in [future-scope](../future-scope.md).
2. **First-run setup screen** (REQ-CFG-018) offers a password (recommended) or open mode. The choice is stored in `admin.json` (schema-versioned, written atomically, mode 0600 on Unix).
3. **Hashing: PBKDF2-HMAC-SHA256 built on `sha2`, which is already in the tree** (REQ-CFG-019). Cost: about 60 lines, no new dependency, no binary growth beyond that code, constant memory (two SHA-256 states). The iteration count (100,000) and salt are stored with the hash so the cost can be raised later without a migration. Alternatives rejected: argon2 or scrypt crates add code and, by design, tens of MiB of working memory, which the budget forbids; a bare SHA-256 of a password (what endpoint tokens use) is fine for 256-bit random tokens but not for human passwords. The trade-off accepted: PBKDF2 is less resistant to GPU cracking than a memory-hard function, so the hash file is protected by file mode, the minimum length is 8, and the attack needs access to the device's disk.
4. **Sessions, not password-per-request.** Verifying costs real CPU on a small core, so sign-in returns a random 256-bit session token kept in memory (hashed, at most 8, 12-hour lifetime). Restarting the core signs everyone out.
5. **Open mode is private-network only** (REQ-CFG-020): it can be chosen, and used, only from loopback, private, link-local, or 100.64.0.0/10 addresses (the last because Tailscale and carrier-grade NAT use it), and never with a specific public listen address. A persistent banner shows in every view. Webhook, bridge, and satellite tokens are separate credentials and remain required.
6. **Setup is reachable before it is done.** On a shared network, whoever opens the page first could choose the password. Mitigations within the budget (REQ-CFG-021): the default listen address is loopback; setup is accepted only from private addresses; and when the device listens beyond loopback, a one-time 8-digit setup code is printed on the console and must be entered. The residual risk is documented in the [threat model](../security/threat-model.md): where the listen address is a LAN address and nobody can see the console, the code is unreadable, so run the first start somewhere you control, or complete setup before exposing the device.
7. **Recovery** is `clawdot --reset-admin` on the device with the server stopped (REQ-CFG-022). Physical or shell access is the root of trust, as with any appliance.
8. **Migration** (REQ-CFG-023): an existing `admin-token.hash` is converted in place; the old token then works in the password field and the UI invites the administrator to set a real password. We chose this over forcing a new setup because re-running setup would briefly open a protected installation to the first visitor on the network.

## Consequences

**Positive:** no secret on the console, no lost-token lockout, one unambiguous recovery path, and no new dependencies.
**Negative:** the setup page is an unauthenticated surface until used; the setup code is useless without a console; PBKDF2 is weaker than a memory-hard function; sessions are lost on restart; a reverse proxy or port-forward on a private address makes remote clients look private (open mode behind a proxy is unsupported and documented as such).

## Revisit when

A memory-hard hash fits the measured budget, or multi-user accounts are promoted from future scope.
