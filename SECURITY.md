# Security Policy

## Reporting a vulnerability

**Do not open a public issue.** Report privately using GitHub's "Report a vulnerability" (Security Advisories) on this repository. If that option is not available to you, contact the maintainer through the address on their GitHub profile.

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

There is no tagged release yet (the version is 0.0.0). Until there is one, only the current `dev` branch receives fixes. After the first release, only the latest release does, until 1.0; from 1.0, the latest minor release and the previous one.

## Scope

In scope: the core (including its web portal, HTTP API, MCP server and updater), the satellite protocol, the reference satellite and device daemon, the agent bridge, and build and release tooling. See the [threat model](docs/security/threat-model.md).

Out of scope: third-party provider services, physical attacks, and deployments exposed directly to the public internet (unsupported).

## Hardening notes for users

- Keep the device on a trusted LAN; use a VPN or tunnel for remote agents. The portal, the API and the satellite port use plain HTTP and TCP, and the portal answers only to the device's own names and addresses.
- Choose the admin password at first run (or deliberately leave the page open on a private network; it shows a banner). Finish setup right after the first start, before others on the network can reach it.
- Use a unique token per inbox endpoint and revoke unused ones. Endpoint tokens are required even when the page is open.
- Remember the admin password; there is no recovery code. Locked out? Hold the speaker's action button for 10 seconds, or run `clawdot --reset-admin` with the server stopped. See [the guide](docs/guides/forgot-password.md).
- Updates are installed only from releases signed with the project key, and only when you confirm.
- Run agents under their own permission policies. Replies spoken to the speaker may influence them.
