# Recipe: Home Assistant

Read a sensor, switch lights you name, or hand a sentence to Home Assistant's own assistant. Optional: ClawDot never needs Home Assistant (REQ-AST-014, spec scope).

File: [`home-assistant-rest.json`](../../examples/recipes/home-assistant-rest.json), using Home Assistant's REST API with a **long-lived access token** in `HA_TOKEN` (create it on your profile page, "Long-lived access tokens"). The REST API takes it as `Authorization: Bearer <token>` (developer documentation, read 2026-11-09).

| Tool | Call | Tier | Why |
|---|---|---|---|
| `home_state` | `GET /api/states/{entity_id}` | `read_only` | Reads one entity. The state and attributes are outside text. |
| `home_light` | `POST /api/services/light/{service}` with `{"entity_id": ...}` | `reversible` | `service` is only `turn_on` or `turn_off`, and `entity_id` only the lights you list in the file. A wrong light is undone by one command. After outside text it is held for a yes. |
| `home_assist` | `POST /api/conversation/process` with `{"text", "language"}` | `costly` | Free-form: Home Assistant may unlock a door or open a garage if you exposed that to Assist. The host reads the exact sentence back and waits for the yes. |

Lights, switches and climate are fine as `reversible` tools. **Do not add locks, garage doors, alarms or water valves as reversible tools**; leave them to `home_assist` (held) or give them their own `costly` tool.

## The Home Assistant MCP server is not usable as it stands

Home Assistant (2025.2 and later) has an MCP server at `/api/mcp`, with per-tool-set URLs such as `/api/mcp/assist`. Its page, read 2026-11-09, describes **OAuth 2.0 with PKCE** and does not mention long-lived tokens; it names no transport, though its Claude Code example uses `--transport http`. ClawDot's MCP client takes only a fixed bearer from the environment and requires MCP revision `2025-06-18` ([mcp.md](../architecture/mcp.md), "OAuth" is not built). So the MCP route is **not offered as a recipe**: it would need either OAuth support in the core or a proxy that adds a token, and the revision Home Assistant negotiates was not checked. Use the REST recipe.

## Setup

1. Copy the file and change `homeassistant.local:8123` to your instance. Plain `http://` is allowed on a private network.
2. Edit the `entity_id` list of `home_light` to the lights you want. Add more tools by copying `home_light` for `switch` or `climate` only if a wrong call is harmless.
3. `export HA_TOKEN=...` in the environment of the ClawDot service.
4. Create a dedicated Home Assistant user for the token and limit what it may do. The token is as powerful as that user, whatever the tiers say.

## Failure modes

| Symptom | Cause |
|---|---|
| `401` | Wrong or expired token, or the user was deleted. |
| `404` on `home_state` | The entity does not exist; ask the model to check the name (it can call `home_assist`). |
| `400` from `home_assist` | The conversation endpoint refused the body. The shape used is `{"text": ..., "language": "en"}`; it comes from the `conversation.process` action's parameters and **was not checked against the REST reference**, which the page read did not document. |
| Assist answers "Sorry, I am not aware of any device called ..." | The entity is not exposed to Assist. That is Home Assistant's setting, not ClawDot's. |
| Reply is cut off | The core keeps 1,000 characters of any response. |
| Model asks twice before turning a light off | The conversation held outside text (a web page), so the light call is held for a yes. |

## Verify

* `scripts/cargo.sh test -p clawdot-core --test recipes`: `home_light` posts `{"entity_id":"light.den"}` to `/api/services/light/turn_on` with the bearer, and refuses a lock, and a `service` of `../../lock/unlock`.
* `python3 examples/recipes/fake_server.py` and press Test on each tool.
* `--dry-run` against the real instance: `home_state` runs, `home_light` and `home_assist` are described aloud and not sent.

## Sources (read 2026-11-09)

* REST API: <https://contributors.home-assistant.io/core/internals/rest-api/> (states, services, calendars; the conversation endpoint is only named there)
* MCP server: <https://www.home-assistant.io/integrations/mcp_server/>

**Not verified:** the `/api/conversation/process` request and response shape, any live call, and the MCP server's revision and transport.
