# Recipes: the default toolset

A recipe is a tested configuration that gives ClawDot one more ability, with no change to the core and nothing hard-coded (REQ-EXT-007). Each is a declarative [HTTP tool](../architecture/http-tools.md), an [MCP connection](../architecture/mcp.md) or an inbox endpoint, and each is optional: the core runs with none of them.

| Recipe | What it gives you | Cost | Default tiers |
|---|---|---|---|
| [Web search](web-search.md) | "What is the weather in Oslo", "who won last night" | SearXNG: free (your own server). Brave and Tavily: free tiers, then paid | `read_only` |
| [Spotify](spotify.md) | Search, play, pause, skip and volume on a Connect device | Needs Premium. **Read the policy section first: Spotify's Developer Policy restricts voice control** | `read_only` for reads, `reversible` for playback |
| [Home Assistant](home-assistant.md) | Read a sensor, switch a named light, ask Assist | Free | `read_only`, `reversible` (lights), `costly` (free-form Assist) |
| [Repository host (GitHub)](github.md) | CI status, open pull requests, re-run failed jobs, open a pull request | Free; re-runs use build minutes | `read_only`, `costly` |
| [Calendar (read-only)](calendar.md) | "What is on tomorrow" | Free | `read_only` |
| [Claude Code hook](claude-code-hook.md) | Claude Code tells the speaker when it finishes or needs you | Free | (inbox events, not tools) |
| [Hermes Agent as the model](hermes-brain.md) | Use Hermes Agent as the brain through its OpenAI-compatible server (written from docs, not run) | Free; your model provider | (a model setting, not a tool; keep Hermes read-only) |

The example files are in [`examples/recipes/`](../../examples/recipes/). Each is a fragment of the configuration file (`tools` or `mcp` keys): merge it into your file, or add the tools in the portal's Tools and Connections tabs.

## Rules every recipe follows

* **Credentials are environment variable names only.** `auth_env` names a variable on the device; the value never appears in a file, the portal, the audit log or a prompt (REQ-SEC-001). The core reads the variable once, at start-up, so a token that expires needs a helper outside the core (the Spotify recipe has one).
* **Every tool states its tier** (REQ-SEC-010). `read_only` runs. `reversible` runs without asking unless the conversation already holds text from outside (a web page, a calendar entry), in which case it is held for a yes (the taint rule, [action-safety.md](../architecture/action-safety.md)). `costly` and `destructive` are always read back and wait for the yes. A recipe never sets a tier lower than the harm of a wrong call: a wrong light is reversible; re-running a build, sending a message or opening a pull request is not.
* **The model chooses parameters, never the destination.** Hosts, paths, headers and keys are fixed. Where a parameter picks a device or a light, the recipe lists the allowed values (`enum`), so the model cannot name one you did not list.
* **Results are data.** Search results, calendar titles, run names and speaker names come from outside. Tool descriptions say so, and the host fences them.
* **The core never talks to the vendor for you beyond the one declared call.** No redirects, no retries, responses clipped to 1,000 characters (a long JSON answer is summarised, [agent-loop.md](../architecture/agent-loop.md)).

## Verify a recipe without an account

Each recipe has a verification section. They all use the same three steps, cheapest first:

1. **The file is valid and its shapes are right.** `scripts/cargo.sh test -p clawdot-core --test recipes` loads every example through the core's own checks and calls every tool against a fake server, comparing the URL, headers and body with what the service documents. `scripts/cargo.sh run -p clawdot-eval -- replay --filter rcp` runs the recipe scenarios (the `rcp-` set) with fixture tools.
2. **The request is what you expect, from your own configuration.** Run the stand-in server, point a copy of the recipe at it, and press Test in the Tools tab:
   ```
   python3 examples/recipes/fake_server.py            # prints every request, hides credentials
   ```
   Change each `url` host to `http://127.0.0.1:9999` in the copy. Nothing leaves your machine.
3. **The real service, with actions switched off.** Start the core with `--dry-run`: read-only tools run for real, and every other call is described aloud and recorded in the audit log without being sent (REQ-SEC-016). Ask for something that acts and check what it says it would do.

## What was and was not verified

The recipes were written against the vendors' public documentation, read on 2026-11-09 (each recipe lists the pages). **No recipe has been run against a live account or server by the authors**: no live model, search, Spotify or Home Assistant call was made. Steps 1 and 2 above prove the request shapes; they cannot prove a vendor's behaviour. Each recipe lists what could not be verified. Record the first real run in [open-questions.md](../open-questions.md).
