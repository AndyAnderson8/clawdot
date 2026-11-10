# Security Policy

## Reporting a vulnerability

**Do not open a public issue.** Report privately using GitHub's "Report a vulnerability" (Security Advisories) on this repository. Until the repository is public, contact the maintainer directly.

Please include: affected version/commit, a description and impact, reproduction steps, and any suggested fix.

## What to expect

| Step | Target |
|---|---|
| Acknowledgement | within 3 business days |
| Initial assessment | within 7 days |
| Fix or mitigation plan | within 30 days for high/critical |
| Coordinated disclosure | agreed with the reporter; credit given unless declined |

Security issues are handled as the **Expedite** class of service ([process](docs/process/sdlc.md)).

## Supported versions

Pre-1.0: only the latest release receives fixes. From 1.0, the latest minor release and the previous one.

## Scope

In scope: the core, protocol, reference satellites, web UI/API, and build/release tooling. See the [threat model](docs/security/threat-model.md).

Out of scope: third-party provider services, physical attacks, and deployments exposed directly to the public internet (unsupported).

## Hardening notes for users

- Keep the device on a trusted LAN; use a VPN/tunnel for remote agents.
- Choose the admin password at first run (or deliberately leave the page open on a private network; it shows a banner). Finish setup right after the first start, before others on the network can reach it.
- Use a unique token per inbox endpoint and revoke unused ones. Endpoint tokens are required even when the page is open.
- Save the recovery code the page shows when you set a password. Locked out? Use it on the sign-in page, hold the speaker's action button for 10 seconds, or run `clawdot --reset-admin` with the server stopped. See [the guide](docs/guides/forgot-password.md).
- Run agents under their own permission policies. Replies spoken to the speaker may influence them.
