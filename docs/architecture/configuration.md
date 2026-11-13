# Configuration

ClawDot reads one human-editable JSON file with `--config FILE` (REQ-CFG-007). Every key is optional. Command-line
flags override the file. Unknown keys and impossible values stop startup with a message that names the key.

```json
{
  "data_dir": "/var/lib/clawdot",
  "listen": "0.0.0.0:3335",
  "public_url": "http://dot.local:3335",
  "zone": "America/Chicago",
  "name": "Kitchen Dot",
  "persona_file": "/etc/clawdot/persona.txt",
  "endpoints": ["claude-code", "ci:generic"],
  "llm": {
    "url": "https://openrouter.ai/api/v1",
    "model": "provider/model",
    "fallback_model": "provider/cheaper-model",
    "summary_model": "provider/cheap-model",
    "key_env": "OPENROUTER_API_KEY"
  },
  "limits": { "daily_spend_usd": 2.0 }
}
```

| Key | Meaning | Default |
|---|---|---|
| `data_dir` | Where state is kept | `./clawdot-data` |
| `listen` | HTTP address; bind to the LAN only (REQ-CFG-005) | `127.0.0.1:3335` |
| `public_url` | URL printed in integration snippets | derived from `listen` (`0.0.0.0` and `[::]` become `clawdot.local`) |
| `zone` | IANA time zone | UTC |
| `name` | Device name shown in the portal header | the host name |
| `persona_file` | Replaces the default system prompt | built-in persona |
| `endpoints` | Inbox endpoints to create at startup (`NAME` or `NAME:ADAPTER`) | none |
| `llm.url`, `llm.model` | Language model; set together or not at all. `https://` is required unless the host is localhost, `.local` or a private-network address (REQ-SEC-001) | none (fast path only) |
| `llm.fallback_model`, `llm.summary_model` | Optional; need `llm.url` and `llm.model` | none |
| `llm.key_env` | Name of the environment variable holding the key | `OPENROUTER_API_KEY` |
| `limits.daily_spend_usd` | Stop calling the model after this much spend. Counted from the cost the provider reports, else from its token counts times the portal settings `llm_usd_per_mtok_in` and `llm_usd_per_mtok_out`, else estimated from sizes at built-in prices of $10 and $40 per million tokens, so enter your prices (REQ-AST-011) | none |
| `limits.max_actions_per_day` | Calls that are not read-only allowed per local day; 0 stops all actions (REQ-SEC-015) | 100 |
| `limits.max_costly_per_day` | Costly and destructive calls allowed per local day (REQ-SEC-015) | 20 |
| `limits.max_requests_per_day` | Retired: there is no cap on the number of requests. Still accepted so an older file loads; it is ignored. Use `daily_spend_usd` | ignored |
| `limits.dry_run` | Describe actions instead of running them; also `--dry-run` (REQ-SEC-016) | off |
| `agent.max_rounds` | Model rounds in one turn; at most 20 (REQ-AST-019) | 8 |
| `agent.max_tool_calls` | Tool calls in one turn; at most 20 | 12 |
| `agent.deadline_secs` | Seconds one turn may take; at most 300 | 60 |
| `agent.tool_timeout_secs` | Seconds one tool call may take before it counts as failed; at most 120 | 45 |
| `agent.plan_speech`, `agent.progress_speech` | Say the plan first, and short progress phrases while tools run (REQ-AST-020) | true, true |
| `agent.summarize_over_chars` | Summarise tool results longer than this for speech; 0 turns it off (REQ-AST-024) | 400 |
| `agent.notes` | Keep the notes the user asks for, in `memory.json` (REQ-AST-023) | false |
| `tools` | Declarative HTTP tools the model may call; see [http-tools.md](http-tools.md) | none |
| `mcp` | MCP servers whose tools the model may call; see [mcp.md](mcp.md) | none |
| `stations` | Internet radio stations for the music commands; see [media.md](media.md) | none |
| `device` | What the device is called and which parts it has that the system cannot show (REQ-CFG-083): `board` (an identifier: lower-case letters, digits, `-` and `_`, at most 40; use the `board` value of the file in `boards/`), `label` (what the portal shows, at most 60 characters on one line) and `parts` (`leds`, `bluetooth`, `wifi`, `screen`, `usb_serial`, each `true`, `false` or absent for as detected). Reported in the status as `hardware.board`, `hardware.label` and `hardware.configured_parts`; see [board-support.md](board-support.md). Ready-made sections are in `examples/devices/` | `board` is `unknown`, no label, every part as detected |
| `speech` | Speech-to-text and text-to-speech provider (`url`, `key_env`, `stt_model`, `tts_model`, `voice`, `language`, `tts_sample_rate`); see [voice-pipeline.md](voice-pipeline.md) | none |
| `voice.realtime` | Optional realtime (speech-to-speech) voice: `enabled`, `provider` (`openai`), `url` (`wss://`), `model`, `voice`, `key_env`, `transcription_model`, `usd_per_min_in`, `usd_per_min_out`, `max_session_secs`. Needs a build with the `realtime` feature; also the portal settings `realtime_enabled`, `realtime_url`, `realtime_model`, `realtime_voice`, `realtime_key_env`, `realtime_transcription_model`, `realtime_usd_per_min_in`, `realtime_usd_per_min_out` and `realtime_max_session_secs`, all applied without a restart (REQ-RTV-008). See [../guides/realtime-voice.md](../guides/realtime-voice.md) | off |
| `satellite` | Where speakers connect (`listen`, `token_env`, `follow_up_secs`); see [satellite-server.md](satellite-server.md) | off |
| `esphome` | ESPHome voice satellites to dial (`devices`, each with `name`, `address`, `psk_env` and `reply`, which is `"whole"` or `"streamed"` and defaults to `"whole"`; `media_listen`, `media_url`); needs a build with the `esphome-satellite` feature; see [esphome-satellite.md](esphome-satellite.md) | off |

Every `limits.*` and `agent.*` row above except `agent.summarize_over_chars` is also a portal setting (Settings, "Agent settings", REQ-CFG-062): `agent_max_rounds`, `agent_max_tool_calls`, `agent_deadline_secs`, `agent_tool_timeout_secs`, `plan_speech`, `progress_speech`, `notes`, `max_actions_per_day`, `max_costly_per_day` and `dry_run`. They follow the usual layers (default, this file, saved in the portal, flag such as `--dry-run`), apply without a restart, and the page shows the value in force and where it came from. The same group holds the device-wide quiet hours (`quiet_start`, `quiet_end`, local `HH:MM`, both or neither; REQ-INB-040), which exist only as a setting: while the window lasts, only urgent agent events are announced.

## Settings saved from the portal

The Settings tab (REQ-CFG-037..042) saves changes to `settings.json` in the data directory (atomic JSON, mode 0600).
Values are layered, lowest first: **built-in defaults, the configuration file, `settings.json`, command-line
flags**. The effective value and the layer it came from are shown next to each setting. A flag beats a saved value;
the page says so. Clearing a saved value (leave the box empty) falls back to the layer below. Unknown keys or a newer
schema in `settings.json` stop startup with a message; a saved value that no longer validates is skipped with a
warning so a headless device still starts.

| Setting | Key in `settings.json` | Applies |
|---|---|---|
| Device name, time zone, personality (the text itself) | `device_name`, `zone`, `persona` | right away |
| Language model address, model, backup model, summary model, key variable name | `llm_url`, `llm_model`, `llm_fallback_model`, `summary_model`, `llm_key_env` | right away (the model client is rebuilt) |
| Daily spend | `daily_spend_usd` | right away |
| Model prices, in dollars per million tokens in and out (0 means free; empty means unknown) | `llm_usd_per_mtok_in`, `llm_usd_per_mtok_out` | right away |
| Voice | `voice` | after a restart (needs the `speech` section of the configuration file) |
| Listen address, public address | `listen`, `public_url` | after a restart |

**How the layers are built.** The command line is parsed into a `Cli` (`cli.rs`), which holds only what was typed, and the `--config` file into a `Config` (`config.rs`). `seed::resolve` turns those two, the built-in defaults and `settings.json` into one `Resolved` value (`seed.rs`): the effective setting values plus the sections of the file that are read directly (`tools`, `mcp`, `stations`, `speech`, `satellite`, `device`, `updates`, `esphome`). It is built once; start-up reads it by reference and changes nothing in it. The file's value for a setting is found through one table, `FILE_KEYS` in `seed.rs`, that gives for each setting key the JSON pointer to its field in the file; a test fails when a field of the `llm`, `limits`, `agent` or `voice` sections, or a top-level key, has no row and is not on the list of keys read directly or of fields that are not settings. To make a setting settable from the file, add the field to `Config`, a row to `settings::FIELDS`, and a row to `FILE_KEYS`.

The former `board` setting (`auto` or `none`) is retired: nothing ever read it. A `settings.json` or an export that still holds it loads, and the value is dropped. What a device is called now comes from the `device` section of the configuration file, which is read at start.

A change is validated as a whole before anything is saved; an error says what is wrong and how to fix it. The page
lists saved values that are not yet running and gives the restart instruction; it has no Restart button because the
core cannot restart itself safely (it may run under systemd, a container, or a terminal). The admin password is
changed separately and is stored hashed in `admin.json`. `PUT /api/settings` needs the administrator, like every
mutating endpoint.

**Moving and diagnosing (About view, REQ-CFG-063 to 066).** `GET /api/settings/export` returns the settings saved on this device as `{"format":"clawdot-settings","version":1,"settings":{...}}`; loading a file is `PUT /api/settings` with its `settings`, so the same all-or-nothing validation applies and a file cannot set anything the page could not. `GET /api/diagnostics` returns one JSON document for a bug report: versions and build information, the settings with their sources (the persona as a length only), webhook names without tokens or hashes, tool and connection counts, audit counts by decision and tier, and recent problems; it contains no secret, audio, spoken words, note text or event text. `GET /api/logs?limit=` returns the core's own recent warnings and errors (`diag.rs`: at most 200 lines of 300 characters, redacted, in memory only, lost at restart). `GET /api/about` also returns the build profile, platform and compiled-in features.

## Rules

- **Secrets are never in the file, and never in `settings.json`.** The API key comes from the environment variable
  named by `llm.key_env` (or the portal's "Key variable" setting, which holds the NAME). Naming a variable that is not
  set is refused with that name. A key can also be pasted into the portal; it is kept in `secrets.json` in the data
  directory (mode 0600, not encrypted), shown only as a masked hint, and never part of an export (`secrets.rs`).
- **The environment is read once**, at the start of `main` (`env::Env::from_process`), and a lookup is passed down; nothing else in the core reads the process environment. A setting that names a variable (`llm_key_env`, `realtime_key_env`, `speech.key_env`, a connection's `auth_env`) resolves the name chosen at that time, in that start-up snapshot. A variable exported after ClawDot started is therefore not seen until it restarts, which was already true of a running process.
- **Validated at startup**, before anything is bound or written. (MCP sources are only declared and checked then; they connect after the listener and the loop start, and a source that cannot be reached is reported, not fatal.)
- **Stopping.** SIGTERM (a service manager's stop), SIGINT and, on Windows, Ctrl-C make the loop do what the console's `quit` does: queue the usage counters, wait until every write queued on the store writer thread is on disk, and exit 0 (`shutdown.rs`, `App::stop_on_signal`, REQ-CFG-085). The wait is at most 5 seconds; if the disk has not answered by then the program says so on standard error and exits anyway. Any other signal, such as SIGKILL, still ends the program at once and loses what was queued.
- **Hot reload** of the configuration file (a SHOULD in the spec) is not implemented: restart to apply file changes.
  Settings saved from the portal apply live where the table above says so. Rotating an endpoint token
  already works without a restart via the HTTP API.

## Web UI

The core serves a small page at `/` and `/ui` (REQ-CFG-001, 008): no external assets, usable on a
phone, as a portal with five destinations (Home, Clock, Media, Logs, Settings, whose pages hold tools, connections, skills, webhooks, notes, voice, updates and the rest; see [design/portal.md](../design/portal.md)) with a header and a version footer (REQ-CFG-025, 026). Webhooks (REQ-CFG-003, 028) create, rotate, and delete endpoints and show copy-paste snippets; the Clock page (REQ-CFG-027) manages timers, alarms, and reminders; tools, connections and settings (REQ-CFG-029, 030) show and change what the device is running with.

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
- **Hold the button** (REQ-CFG-069, REQ-DEV-031). Holding the action button for 10 seconds (or typing `admin` in the
  desktop device console) opens a 5 minute window in which the sign-in page offers a new password. There is no recovery code.
  Private networks only, one use. The core honours it only from a satellite on the device itself or one with a valid
  satellite token (REQ-SAT-020).
- **Recovery by command** (REQ-CFG-022). On a PC or server, stop the server and run `clawdot --reset-admin` (with the
  same `--data-dir`). It clears only the administrator setting; endpoints and the rest of your state stay. The next
  start shows setup again. The page itself never tells anyone to run a command (REQ-CFG-071). The guide for every
  device type is [forgot-password.md](../guides/forgot-password.md).
- **Upgrading** (REQ-CFG-023). If `admin-token.hash` exists from an earlier version, it is converted on the first
  start: the old token keeps working, typed into the password field, and the page invites you to set a real
  password (which retires the token). `--rotate-admin-token` is gone; use `--reset-admin`.
- **Host names.** The portal page and its sign-in-free routes answer only to the device's own names: an IP address, `localhost`, the host of `public_url` and the machine's host name (`hostguard.rs`, threat T5). Behind a reverse proxy or under another name, set `public_url`. A browser request to `/api` that changes something is refused when its `Origin` is not the host it was sent to, and any `/api` request a browser marks `Sec-Fetch-Site: cross-site` is refused. Scripts send neither header and are not affected.
- **Binding.** The default listen address is `127.0.0.1:3335` (not 8080, which Amazon's `oobed` service holds on an Echo Dot; REQ-CFG-014). Change `listen` to the LAN address deliberately;
  there is no TLS, so use a trusted network or a VPN (REQ-CFG-005).
- **Start-up order.** The listening address is bound first; only then is the administrator
  setting written (REQ-CFG-015). `--create-endpoint` and `--rotate-endpoint` print their token and exit without starting
  the server; run them with the server stopped, then start it. Snippets use `--public-url`, or a URL derived from
  `listen` (REQ-CFG-016).
- **Console.** Type `quit` or `exit`, or press Ctrl-D, to stop (REQ-CFG-017).
- **Throttling.** Wrong sessions, wrong tokens, sign-in attempts, setup tries and wrong "current password" guesses are counted per client address (20 at once, then 1 a second, for at most 64 addresses at a time), under a larger ceiling for all addresses together (100, then 5 a second). A right password gives its attempt back. A change that could not be written to storage is answered with an error (500, or 507 when the storage is full), never as a success.
- **Untrusted text.** Agent text is rendered as text only, and the page carries a strict Content-Security-Policy.
- **Status** shows the model and today's request count, connected satellites, how many tools are loaded (built in, HTTP, MCP), and the clock state. It contains no secrets and no agent text. Radio and music are not shown in the portal.
- **Test speak** makes the speaker say a short phrase (up to 200 characters, a few at a time) on the console and on any free satellite, a quick way to check the speaker end to end.
- Not in the UI: editing the configuration file itself, hot reload of it, and wake word settings. Settings, provider keys, logs, notes, tools, connections and skills are managed from the portal (REQ-CFG-032..036, 062..064). A tool or connection is edited by deleting and adding it again.
