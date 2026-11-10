# ADR-0013: First-run administrator setup instead of a printed token

- **Status:** Proposed
- **Date:** 2026-11-08
- **Deciders:** Maintainer

## Context

The first web UI printed an administrator token once on the console and asked for it on a "Sign in" page. On a headless speaker there is often no console to read, a lost token needs shell access to replace, and the token is awkward to type on a phone. The maintainer wants a normal first-run experience: choose a password, or deliberately leave the UI open on a trusted network. The device must stay within the Echo Dot Gen 2 budget ([ADR-0003](0003-reference-hardware-budget.md)), and the UI must not become a way onto the LAN for strangers.

## Decision

1. **One administrator, one password. No accounts.** Multi-user admin stays parked in [future-scope](../future-scope.md).
2. **First-run setup screen** (REQ-CFG-018) offers a password (recommended) or open mode. The choice is stored in `admin.json` (schema-versioned, written atomically, like the other state files).
3. **Hashing: PBKDF2-HMAC-SHA256 built on `sha2`, which is already in the tree** (REQ-CFG-019). Cost: about 60 lines, no new dependency, no binary growth beyond that code, constant memory (two SHA-256 states). The iteration count (100,000) and salt are stored with the hash so the cost can be raised later without a migration. Alternatives rejected: argon2 or scrypt crates add code and, by design, tens of MiB of working memory, which the budget forbids; a bare SHA-256 of a password (what endpoint tokens use) is fine for 256-bit random tokens but not for human passwords. The trade-off accepted: PBKDF2 is less resistant to GPU cracking than a memory-hard function, so the hash file is protected by file mode, the minimum length is 8, and the attack needs access to the device's disk.
4. **Sessions, not password-per-request.** Verifying costs real CPU on a small core, so sign-in returns a random 256-bit session token kept in memory (hashed, at most 8, 12-hour lifetime). Restarting the core signs everyone out.
5. **Open mode is private-network only** (REQ-CFG-020): it can be chosen, and used, only from loopback, private, link-local, or 100.64.0.0/10 addresses (the last because Tailscale and carrier-grade NAT use it), and never with a specific public listen address. A persistent banner shows in every view. Webhook, bridge, and satellite tokens are separate credentials and remain required.
6. **Setup is reachable before it is done.** On a shared network, whoever opens the page first could choose the password. Mitigations within the budget (REQ-CFG-021): the default listen address is loopback; setup is accepted only from private addresses; and when the device listens beyond loopback, a one-time 8-digit setup code is printed on the console and must be entered *(superseded: see the amendment below)*. The residual risk is documented in the [threat model](../security/threat-model.md): where the listen address is a LAN address and nobody can see the console, the code is unreadable, so run the first start somewhere you control, or complete setup before exposing the device.
7. **Recovery** is `clawdot --reset-admin` on the device with the server stopped (REQ-CFG-022). Physical or shell access is the root of trust, as with any appliance.
8. **Migration** (REQ-CFG-023): an existing `admin-token.hash` is converted in place; the old token then works in the password field and the UI invites the administrator to set a real password. We chose this over forcing a new setup because re-running setup would briefly open a protected installation to the first visitor on the network.

## Consequences

**Positive:** no secret on the console, no lost-token lockout, one unambiguous recovery path, and no new dependencies.
**Negative:** the setup page is an unauthenticated surface until used; the setup code is useless without a console; PBKDF2 is weaker than a memory-hard function; sessions are lost on restart; a reverse proxy or port-forward on a private address makes remote clients look private (open mode behind a proxy is unsupported and documented as such).

## Revisit when

A memory-hard hash fits the measured budget, or multi-user accounts are promoted from future scope.

## Amendment 2026-11-08: the setup code is removed

Decision point 6 is superseded. The reference device has no visible console, so a setup code printed there makes first-run setup impossible on the very hardware this project targets (found in hardware testing). The code, the code field in the setup page, and the `code` parameter of the setup API are removed (REQ-CFG-021 v0.10).

What remains of point 6: the default listen address is loopback; setup is accepted only from loopback, private, link-local, or 100.64.0.0/10 peers; open mode keeps its private-address rules and the persistent banner (REQ-CFG-020); webhook, bridge, and satellite tokens stay required; `--reset-admin` still recovers.

**Accepted residual risk:** until setup completes, whoever on a private network reaches the page first can set the administrator password or choose open mode. The console prints the URL and a one-line warning at start-up. Mitigation is procedural: complete setup right after the first start on a network you trust, and keep the default loopback listen address until you are ready. Threat T15 records this. The original text above is kept as history.

## Amendment 2026-11-09: recovery without a shell

Decision point 7 (recovery is `clawdot --reset-admin`) assumed a shell. A smart speaker has none, and the portal told people to run a command they cannot run. Recovery is now layered (REQ-CFG-067..072, REQ-DEV-031, REQ-SAT-020); `--reset-admin` stays for PCs and servers.

1. **Recovery code.** Setting a password issues a one-time code (20 characters from 31 look-alike-free symbols, about 99 bits, shown as four groups of five). Only a salted PBKDF2 hash, with the password's parameters, is stored in `admin.json`. The code resets the password without a sign-in; it is single use and a new one is issued and shown once after each reset. Existing installs get one from Settings; `admin.json` gains an optional `recovery` field and old files load unchanged.
2. **Local reset window.** A device that has an action button asks the core (`clawdot.admin-reset`) after a 10 second hold. The core opens a 5 minute, single-use, in-memory window in which a private-address peer can set a new password without a code, then issues a new code. Physical presence at the device is the root of trust, as it was for the shell.
3. **Open mode and setup are untouched**: no code, no window.

### Security review

- **Brute force.** The code is about 99 bits. `POST /api/recover` spends an attempt of the shared authentication rate limit before any hashing, so a flood cannot force expensive work and, once the budget is gone, even a correct code gets `429`. Verification is a constant-time compare of PBKDF2 outputs. Every failure (wrong, malformed, no code stored) has one text and status. The rate limit is shared with sign-in, so a flood can also delay a legitimate sign-in; that already held for sign-in and is accepted.
- **Replay.** A code works once. The reset replaces the password and the code hash in one atomic save, and the check re-compares the hash that was verified with the stored one, so two requests racing with the same code cannot both succeed.
- **Sessions.** Any reset ends every session, so a stolen session does not survive a recovery.
- **Window abuse by a guest on the LAN.** A guest could (a) pretend to be a speaker, or (b) press the button. For (a) the core honours `clawdot.admin-reset` only from a satellite on the loopback address (the daemon on the same device) or one that presented a valid satellite token, and not more often than once in 5 seconds; a networked satellite without a token is refused and the core is not asked. For (b) the guest must hold the button for 10 seconds at the device, the device shows the answer with a tone and light, and the window is 5 minutes, single use, closed on restart, private addresses only, with a 60 second cool-down after it ends. **Residual risk:** on a multi-user host, a local process can connect to the loopback satellite port and open the window; treat local users as trusted, as with `--reset-admin` (a satellite token does not help here: loopback is trusted by design). A person at the page during the window can set the password before the owner does; the device announces the window and a reset leaves a log line and an audit record.
- **Logging.** Codes are never logged, audited, exported, put in diagnostics or returned again. The problem log and the audit trail record only the event ("recovery code used", "reset window opened/used/refused"). A response that carries a new code is `no-store` like every API reply.
- **Downgrade.** A file written by this version still signs in under an older core (the extra field is ignored and dropped at its next save, which only means "no code"). A file without the field is "no code", never "an empty code that matches": `recovery_hash` has no value to compare against and the answer is the generic refusal.
- **Weakness accepted.** The code is displayed in the browser tab and exists on screen until the person confirms; it is held in page memory only (not in session storage). Anyone shoulder-surfing at that moment, or any script injected into the page, could read it; the page's CSP forbids external scripts and agent text is rendered as text.
