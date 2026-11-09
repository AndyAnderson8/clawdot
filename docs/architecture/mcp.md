# MCP client

External [Model Context Protocol](https://modelcontextprotocol.io) servers can add tools to the assistant with no
change to the core (REQ-AST-005): a Home Assistant MCP server, a calendar, a notes app. ClawDot is a client only.
It does not expose an MCP server of its own (REQ-EXT-006, later).

- **Transport: Streamable HTTP only.** The core never starts a process, so there is no stdio support
  (REQ-EXT-001). Replies may be plain JSON or a server-sent event stream; both work.
- **Pinned revision: `2025-06-18`.** A server that negotiates a different revision is skipped with a warning that
  names both. Update the pin deliberately, after reading the newer revision, not automatically.

```json
"mcp": [
  {
    "name": "home",
    "url": "http://homeassistant.local:8123/mcp",
    "auth_env": "HA_TOKEN",
    "confirm": true,
    "no_confirm": ["get_state"],
    "only": ["get_state", "turn_on", "turn_off"],
    "timeout_secs": 10
  }
]
```

| Field | Meaning |
|---|---|
| `name` | Short prefix for this server's tools: lowercase letters, digits, `-`, `_`. A tool `turn_on` appears to the model as `home__turn_on`. |
| `url` | The server's MCP endpoint (`http://` or `https://`). |
| `auth_env` | Environment variable holding a bearer token. Never write the token in the file. |
| `confirm` | Default `true`: every tool is `costly`, so the host asks and waits for the user's spoken yes (REQ-AST-008, REQ-SEC-010). `false` makes the server's tools `reversible`. |
| `tiers` | Per-tool tier by the server's own names, for example `{"get_state": "read_only"}`: `read_only`, `reversible`, `costly` or `destructive`. Wins over `confirm` and `no_confirm`. What the server says about itself (such as a read-only hint) is never used. |
| `no_confirm` | Tools (by the server's own names) that never need confirmation even when `confirm` is true, for read-only tools. |
| `only` | If set, only these tools are offered to the model. Use it to expose a small, deliberate surface. |
| `timeout_secs` | 1 to 60, default 10, per request. |

## Treat the server as untrusted

The server is one you chose, but its tool names, descriptions, schemas, and results are data the core does not
control, and a compromised server could try to steer the model. So:

- Tool names are sanitised and prefixed, so a server cannot shadow a built-in tool or another server's tools
  (a clash is skipped with a warning).
- Descriptions are cut to 300 characters; tools with a schema over 8 KB, a non-object schema, or beyond 32 per
  server are dropped.
- Confirmation is on by default and held by the host, not by the prompt: the call waits for a later user turn that says yes (see [action-safety.md](action-safety.md)). The model is given no `confirmed`
  flag and an undeclared one is dropped, so the server never sees it.
- Results are cut to 1000 characters; images and other non-text content become a placeholder such as
  `[image content omitted]`. A result is data for the model, never an instruction to it.
- Startup is not blocked by a broken server: it is skipped with a warning and the rest keep working. An expired
  session is renewed once automatically.

## Not built yet

- Resources, prompts, sampling, and server-initiated requests or notifications (only tools are used).
- Re-listing the tools of a server that is already connected: a server added in the portal is listed once when it is added; if its tools change, remove it and add it again, or restart.
- OAuth. Use a static bearer token from the environment, or a small proxy that adds one.
- Editing an MCP server in place (remove it and add it again). Adding, removing, testing, and switching servers and their tools is in the Connections tab (REQ-CFG-033); see [extensibility.md](extensibility.md).
- Tested only against a fake server in this repository; no real MCP server has been tried.
