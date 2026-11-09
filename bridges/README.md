# Bridges: ntfy and Discord to the speaker

Small programs that run **off the device** and make ClawDot say what happens elsewhere: a push notification on an ntfy topic, a message in a Discord channel. Each subscribes to its service and posts to ClawDot's [announce API](../docs/guides/announce.md) with an endpoint token, so quiet hours, priority, "From <source>" naming, the audit log and the untrusted-text rules all apply on the ClawDot side (REQ-INB-022). Requirements: REQ-INB-028 to REQ-INB-031.

They are kept out of the core on purpose: the default build stays inside the Echo Dot budget, and a chat library never runs on the speaker.

| Bridge | Run | Needs | Dependencies |
|---|---|---|---|
| ntfy | `python3 -m clawdot_bridges.ntfy --config ntfy.toml` | `CLAWDOT_TOKEN`; `NTFY_TOKEN` or `NTFY_USER` and `NTFY_PASSWORD` for a protected server | none (Python 3.11 standard library) |
| Discord | `python3 -m clawdot_bridges.discord_bridge --config discord.toml` | `CLAWDOT_TOKEN`, `DISCORD_TOKEN` | `discord.py==2.7.0`, pinned in [`requirements-discord.txt`](requirements-discord.txt) |

Discord's gateway needs WebSockets, heartbeats and resumption, which is why that one bridge uses a well-known library (MIT licence) and the ntfy bridge, which is a plain HTTP stream, does not.

## What they do, and refuse to do

* **Nothing by default.** A bridge with no `topics` or `channels` does not start. Only what the settings list is spoken. Optional lists narrow it further (authors, keywords, a minimum priority, "only when the bot is mentioned").
* **No raw markup.** Text is cleaned before it is posted: HTML, Markdown, links, mentions, custom emoji, code blocks and control or direction-changing characters are removed, and the length is bounded. People's and channels' names get the same treatment. ClawDot cleans the text again; this is the first line, not the only one ([sanitize.py](clawdot_bridges/sanitize.py)).
* **The source is yours.** The spoken "From ..." name is set in the settings file. A message cannot choose it, so a Discord user named "ClawDot" cannot make the speaker sound like the assistant.
* **Rate limits.** A sustained rate (default 6 a minute) with a small burst (3). Over the limit a message is dropped and counted in the log, never queued, so a flood cannot become a backlog. Identical messages within two minutes are spoken once.
* **Stale messages expire.** Each post carries `ttl_secs` (default 900), after which ClawDot drops it unspoken.
* **Urgency is capped.** `urgent` is spoken in quiet hours, so a bridge only asks for it if `max_priority = "urgent"` is set on purpose. ntfy's priority 5 and any Discord `urgent_keywords` are then mapped to it; by default they are spoken as normal.
* **Secrets only in the environment.** A settings file with a key that looks like a secret (`token`, `password`, `secret`) is refused. The token appears in an `Authorization` header only; errors and logs never contain it.
* **A wrong token stops the bridge** (exit 1, and the service units do not restart on it) instead of retrying forever against the speaker.

## Set up

1. Create an endpoint for the bridge on the ClawDot device, so you can mute or revoke it alone: `clawdot --create-endpoint ntfy:generic`. The token is shown once. Set quiet hours and a minimum priority on the endpoint in the portal if you want them.
2. Copy [`examples/ntfy.toml`](examples/ntfy.toml) or [`examples/discord.toml`](examples/discord.toml), edit it, and keep it outside the repository.
3. Export the secrets, run the command above. For the Discord bridge also: `python3 -m venv venv && venv/bin/pip install -r requirements-discord.txt`.

### ntfy

Use your own ntfy server if you can: on the public `ntfy.sh` the topic name is the only secret. The bridge opens `GET <server>/<topics>/json` (a stream with one JSON message per line), ignores everything but `message` events, and after a dropped connection reconnects with `since=<last message id>` so a gap does not repeat or lose messages. The reconnect delay grows from 1 s to 60 s. Authentication: `NTFY_TOKEN` is sent as `Authorization: Bearer ...`, `NTFY_USER` and `NTFY_PASSWORD` as Basic. The Bearer form for access tokens was **not confirmed** on the ntfy pages read; if a protected server refuses the token, use user and password.

### Discord

1. In the [developer portal](https://discord.com/developers/applications) create an application and a bot; copy the bot token into `DISCORD_TOKEN`.
2. On the bot page turn on the privileged **Message Content** intent. Without it Discord sends empty message text (discord.py documentation). Bots in more than 100 servers need Discord's verification for it; this bridge is meant for your own server.
3. Invite the bot to your server with permission to read the channels you list.
4. Put the channel ids in `channels` (Developer Mode, then right-click a channel, Copy Channel ID).

The bridge only reads. It never writes to Discord.

## Run it as a service

| Where | File |
|---|---|
| Linux (systemd) | [`deploy/clawdot-ntfy.service`](deploy/clawdot-ntfy.service), [`deploy/clawdot-discord.service`](deploy/clawdot-discord.service): secrets in a root-only `EnvironmentFile`, a dynamic user, no new privileges, a memory limit |
| Windows (Task Scheduler) | [`deploy/windows-task.ps1`](deploy/windows-task.ps1): registers a logon task; secrets are user environment variables, not part of the task |
| Docker | [`deploy/Dockerfile`](deploy/Dockerfile), [`deploy/compose.yaml`](deploy/compose.yaml): read-only filesystem, no capabilities, secrets from `env_file` |

## Test and lint

```
python3 -B -m unittest discover -s bridges/tests -t bridges   # from the repository root; stdlib only, loopback only
pip install -r bridges/requirements-dev.txt                    # ruff, pinned
ruff check --config bridges/ruff.toml bridges examples && ruff format --check --config bridges/ruff.toml bridges examples
```

The tests start fake ClawDot and ntfy servers on the loopback address and drive the real code: the request ClawDot receives, cleaning of hostile text, filters, the rate limit, repeats, reconnection with `since`, the refused-token stop, and that no secret reaches a body, a log or an error. The Discord library is not imported by the tests; the decisions are plain Python ([`Rules`](clawdot_bridges/discord_bridge.py)). `scripts/check.sh` runs these tests and ruff when they are available.

**Not tested:** a real ntfy server, a real Discord gateway, `discord.py` itself (the glue in `run()` that turns a Discord message into the plain `Incoming` record), the systemd, Task Scheduler and Docker files, and ruff on this code (it was not installed where this was written; the config is ready).
