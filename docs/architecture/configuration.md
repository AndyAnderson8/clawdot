# Configuration

ClawDot reads one human-editable JSON file with `--config FILE` (REQ-CFG-007). Every key is optional. Command-line
flags override the file. Unknown keys and impossible values stop startup with a message that names the key.

```json
{
  "data_dir": "/var/lib/clawdot",
  "listen": "0.0.0.0:3335",
  "public_url": "http://dot.local:3335",
  "zone": "America/Chicago",
  "persona_file": "/etc/clawdot/persona.txt",
  "endpoints": ["claude-code", "ci:generic"],
  "llm": {
    "url": "https://openrouter.ai/api/v1",
    "model": "provider/model",
    "fallback_model": "provider/cheaper-model",
    "summary_model": "provider/cheap-model",
    "key_env": "OPENROUTER_API_KEY"
  },
  "limits": { "max_requests_per_day": 500, "daily_spend_usd": 2.0 }
}
```

| Key | Meaning | Default |
|---|---|---|
| `data_dir` | Where state is kept | `./clawdot-data` |
| `listen` | HTTP address; bind to the LAN only (REQ-CFG-005) | `127.0.0.1:3335` |
| `public_url` | URL printed in integration snippets | derived from `listen` (`0.0.0.0` and `[::]` become `clawdot.local`) |
| `zone` | IANA time zone | UTC |
| `persona_file` | Replaces the default system prompt | built-in persona |
| `endpoints` | Inbox endpoints to create at startup (`NAME` or `NAME:ADAPTER`) | none |
| `llm.url`, `llm.model` | Language model; set together or not at all. `https://` is required unless the host is localhost, `.local` or a private-network address (REQ-SEC-001) | none (fast path only) |
| `llm.fallback_model`, `llm.summary_model` | Optional; need `llm.url` and `llm.model` | none |
| `llm.key_env` | Name of the environment variable holding the key | `OPENROUTER_API_KEY` |
| `limits.max_requests_per_day` | Model requests per local day, at least 1 | 500 |
| `limits.daily_spend_usd` | Stop calling the model after this much spend | none |
| `tools` | Declarative HTTP tools the model may call; see [http-tools.md](http-tools.md) | none |
| `mcp` | MCP servers whose tools the model may call; see [mcp.md](mcp.md) | none |
| `stations` | Internet radio stations for the music commands; see [media.md](media.md) | none |
| `speech` | Speech-to-text and text-to-speech provider (`url`, `key_env`, `stt_model`, `tts_model`, `voice`, `language`, `tts_sample_rate`); see [voice-pipeline.md](voice-pipeline.md) | none |
| `satellite` | Where speakers connect (`listen`, `token_env`, `follow_up_secs`); see [satellite-server.md](satellite-server.md) | off |

## Rules

- **Secrets are never in the file.** The API key comes from the environment variable named by `llm.key_env`.
- **Validated at startup**, before anything is bound or written.
- **Hot reload** (a SHOULD in the spec) is not implemented: restart to apply changes. Rotating an endpoint token
  already works without a restart via the HTTP API.

## Web UI

The core serves a small page at `/` and `/ui` (REQ-CFG-001, 008): no external assets, no frameworks, usable on a
phone, as a tabbed portal (Home, Reminders, Agents, Tools, Connections, Skills, Webhooks, Settings; see [design/portal.md](../design/portal.md)) with a header and a version footer (REQ-CFG-025, 026). Webhooks (REQ-CFG-003, 028) create, rotate, and delete endpoints and show copy-paste snippets; Reminders (REQ-CFG-027) manages timers, alarms, and reminders; Tools, Connections, and Settings (REQ-CFG-029, 030) show what the device is running with.

- **First run** (REQ-CFG-018). Start `clawdot`, then open the address it prints. While nothing is configured the
  page shows one setup screen with two choices: **protect with an admin password** (recommended), or **leave the
  page open** on this network. There is one administrator and no user accounts.
- **Password mode** (REQ-CFG-019). Only a salted PBKDF2-HMAC-SHA256 hash (100,000 iterations) is stored, in
  `admin.json` in the data directory (REQ-CFG-006); [ADR-0013](../adr/0013-first-run-admin-setup.md) explains the
  choice. Signing in returns a random session token that lives in the browser tab's session storage for up to 12
  hours (at most 8 sessions; a restart signs everyone out). The password is 8 to 128 characters and can be changed
  from the page at any time (REQ-CFG-024).
- **Open mode** (REQ-CFG-020). Anyone on your network can use the page and read your inbox. It can be chosen,
  and is served, only to loopback, private, link-local, and `100.64.0.0/10` (Tailscale) addresses, and never when
  `listen` is a specific public address. Every view shows a red "Unprotected" banner. Webhook, bridge, and
  satellite tokens are separate and **stay required** in both modes. Do not put open mode behind a reverse proxy
  or port forward: the proxy's address looks private to ClawDot. You can set a password later from the same page.
- **Before setup is done** (REQ-CFG-021). There is no setup code, because an Echo Dot has no visible console. The
  setup page is reachable by whoever gets there first, so until you finish it, anyone on a private network who
  opens the page can set the password or choose open mode. Mitigations: the default `listen` is loopback only;
  setup is accepted only from private addresses; the console prints the URL and a warning. Finish setup right after
  the first start, on a network you trust (threat T15, ADR-0013 amendment).
- **Recovery** (REQ-CFG-022). Forgot the password, or want to start over? On the device, stop the server and run
  `clawdot --reset-admin` (with the same `--data-dir`). It clears only the administrator setting; endpoints and
  the rest of your state stay. The next start shows setup again.
- **Upgrading** (REQ-CFG-023). If `admin-token.hash` exists from an earlier version, it is converted on the first
  start: the old token keeps working, typed into the password field, and the page invites you to set a real
  password (which retires the token). `--rotate-admin-token` is gone; use `--reset-admin`.
- **Binding.** The default listen address is `127.0.0.1:3335` (not 8080, which Amazon's `oobed` service holds on an Echo Dot; REQ-CFG-014). Change `listen` to the LAN address deliberately;
  there is no TLS, so use a trusted network or a VPN (REQ-CFG-005).
- **Start-up order.** The listening address is bound first; only then is the administrator
  setting written (REQ-CFG-015). `--create-endpoint` and `--rotate-endpoint` print their token and exit without starting
  the server; run them with the server stopped, then start it. Snippets use `--public-url`, or a URL derived from
  `listen` (REQ-CFG-016).
- **Console.** Type `quit` or `exit`, or press Ctrl-D, to stop (REQ-CFG-017).
- **Throttling.** Wrong sessions, sign-in attempts, and setup tries share the authentication rate limit.
- **Untrusted text.** Agent text is rendered as text only, and the page carries a strict Content-Security-Policy.
- **Status** shows the model and today's request count, connected satellites, how many tools are loaded (built in, HTTP, MCP), and the clock state. It contains no secrets and no agent text. Radio and music are not shown in the portal.
- **Test speak** makes the speaker say a short phrase (up to 200 characters, a few at a time) on the console and on any free satellite, a quick way to check the speaker end to end.
- Not yet in the UI: editing settings (the Settings tab shows them; change the file and restart), adding or removing tools, connections, and skills (REQ-CFG-031 coming-soon states), provider keys, wake word settings, logs, and history search.
