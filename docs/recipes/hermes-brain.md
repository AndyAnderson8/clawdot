# Recipe: Hermes Agent as the language model

Use [Hermes Agent](https://github.com/NousResearch/hermes-agent) (Nous Research, MIT licence) as ClawDot's brain by pointing the language-model address at the OpenAI-compatible server Hermes provides. ClawDot stays the voice front end, the device, the safety checks and the portal. The reasoning and the other link designs are in [R-12](../research/12-agent-harness-brains.md).

**Status: written from documentation, not run.** No Hermes instance was started and no request was made. Read "What could not be verified" before relying on it.

Risk tier: not applicable to this recipe as such (it is a model, not a tool). What matters is what Hermes may do on its own, because **ClawDot's tiers, daily caps and audit log do not see Hermes's own tool use**. This recipe therefore gives Hermes read-only toolsets only.

## Setup

1. Install Hermes on a computer or home server (not on the speaker) following its [quick start](https://hermes-agent.nousresearch.com/docs/getting-started/quickstart), and choose a model with at least 64,000 tokens of context (Hermes refuses smaller ones).
2. Make a **separate Hermes profile for the speaker** with only read-only toolsets: web search, memory and skills reading. No terminal, no file writing, no messaging, no cron creation. `GET /v1/toolsets` on the running server lists the toolset names; set the profile's tools in its `config.yaml` (the Hermes configuration page lists the keys; `platform_toolsets` and `agent.disabled_toolsets` are the ones the ACP page names). Keep `approvals.mode` at `manual` or `smart`; do not use `off` or `--yolo`.
3. Turn on the API server in the Hermes `.env` and start the gateway. The variable names are from the [API server page](https://hermes-agent.nousresearch.com/docs/user-guide/features/api-server):
   ```
   API_SERVER_ENABLED=true
   API_SERVER_HOST=0.0.0.0        # default is 127.0.0.1; use the machine's LAN address, never a public one
   API_SERVER_PORT=8642
   API_SERVER_KEY=<a long random value>
   ```
   ```
   hermes gateway
   ```
   Check it from the ClawDot machine: `curl -H "Authorization: Bearer $API_SERVER_KEY" http://hermes.lan:8642/v1/models`.
4. On the ClawDot device put the same key in an environment variable (never in the file) and set the model section. `llm.url` is the base path; ClawDot adds `/chat/completions`. Plain `http://` is accepted for a private-network address (REQ-SEC-001):
   ```json
   {
     "llm": {
       "url": "http://hermes.lan:8642/v1",
       "model": "hermes-agent",
       "key_env": "HERMES_API_KEY"
     }
   }
   ```
   The model name is the server's `API_SERVER_MODEL_NAME` (default: the profile name, or `hermes-agent`); `GET /v1/models` shows the exact value. The same fields are in the portal's Settings. There is no fallback or summary model unless you also set `llm.fallback_model` and `llm.summary_model`; leave them unset to keep summaries on Hermes too, or point them at a small ordinary model to keep announcements cheap and tool-free.

No example JSON file is shipped for this recipe: the recipe tests (`crates/core/tests/recipes.rs`) load every file in `examples/recipes/` as a tool or connection fragment, and this is a model setting.

## What you get

* "Ask" questions answered with Hermes's search, memory and skills. Hermes's skills (`SKILL.md` folders) and its searchable history are reachable without any ClawDot change.
* ClawDot's own tools (timers, notes, the fast path, recipes) are still ClawDot's. The fast path handles timers and volume before the model is called at all (REQ-AST-010).

## Safety notes

* Everything Hermes does with its own tools is outside ClawDot's confirmation, caps, taint rule and audit log. Do not give the profile a terminal, file writes, email, or a cron-creation tool. Hermes's own approval prompts go to its own channel (CLI or chat), not to the speaker; over the chat-completions endpoint the documentation does not say they can be shown at all.
* The Hermes key gives the holder the same powers as the profile has. Bind the server to the LAN only, keep CORS off (the default), and do not forward the port.
* Hermes's reply text is the assistant's answer. ClawDot still treats text it gets from tools and events as data; it cannot know what Hermes read on the web, so web search results can steer Hermes's reply. This is the usual prompt-injection risk of any search-capable model.

## Failure modes

| Symptom | Cause |
|---|---|
| "I can't reach my model" or a long wait, then an error | The server is not running, `API_SERVER_HOST` is still `127.0.0.1`, a firewall blocks 8642, or the agent took longer than ClawDot's overall limit for one model call. |
| `401` | `API_SERVER_KEY` and the value in `HERMES_API_KEY` differ, or the variable is not in the environment ClawDot started with. |
| `404` on the model | The `model` value is not what `GET /v1/models` returns. |
| `429` | More than `gateway.api_server.max_concurrent_runs` runs (default 10). |
| Timers or tools do not work, Hermes answers in words | Hermes ran the request itself and ignored ClawDot's `tools` list. Expected from the documentation; the fast path still handles timers, and ClawDot's tools are best used with an ordinary model. |
| `409` | A session id belongs to another Hermes surface. ClawDot sends no session header, so this should not happen; it means a proxy added one. |

## Verify

1. No account needed: `scripts/cargo.sh test -p clawdot-core` covers the request ClawDot sends (`POST {url}/chat/completions`, stream on, `tools` and `tool_choice`), and the stand-in server in `examples/recipes/fake_server.py` can be used to see the request, with the `llm.url` host changed to `http://127.0.0.1:9999`.
2. Real server, nothing acting: start ClawDot with `--dry-run`, ask a factual question, then "set a timer for one minute" and "turn on the kitchen light", and note which are handled by ClawDot and which by Hermes. Record the time from the end of your speech to the first spoken word.

## What could not be verified

Read from the Hermes documentation on 2026-11-11 (release v0.23.1); **none of it was run**:

* Whether the chat-completions endpoint accepts and honours a client `tools` array, and what it does with `tool_choice`.
* Whether an approval over this endpoint is shown, denied or hangs.
* Memory and CPU use of Hermes, and the time to the first token when it uses tools.
* ClawDot's overall deadline for one model call against a slow agent (the setting was not found in the configuration table).
* The exact toolset names for a read-only profile; the page names the keys, not a ready-made read-only preset.

Record the first real run in [open-questions.md](../open-questions.md).
