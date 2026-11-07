# HTTP tools

An HTTP tool lets the assistant do something in the world with a single web call: turn on a light through Home
Assistant, trigger a CI job, start a shell script behind a small webhook. It is declared in the configuration
file (REQ-AST-014). The core never runs third-party code (REQ-EXT-001); it only makes the call you described.

```json
"tools": [
  {
    "name": "set_light",
    "description": "Turn a room's light on or off",
    "method": "POST",
    "url": "http://homeassistant.local:8123/api/services/light/{service}",
    "auth_env": "HA_TOKEN",
    "params": {
      "service": { "type": "string", "enum": ["turn_on", "turn_off"], "required": true },
      "entity_id": { "type": "string", "description": "for example light.den", "required": true }
    },
    "confirm": true
  }
]
```

| Field | Meaning |
|---|---|
| `name`, `description` | What the model sees. Names use `a-z`, `0-9`, `_` and may not reuse a built-in tool's name. |
| `method` | `GET` (default), `POST`, `PUT`, or `DELETE`. |
| `url` | Fixed. `{param}` placeholders are allowed in the path and query, never in the host. A placeholder must be a required parameter. |
| `headers` | Extra fixed headers. |
| `auth_env` | Environment variable holding a bearer token. The value is read at startup and never written to the configuration file. |
| `params` | What the model may fill in: `type` (`string`, `integer`, `number`, `boolean`), `description`, `enum` (strings only), `required`. |
| `confirm` | Default `true`: the assistant must ask the user and get a spoken yes before the call runs. |
| `timeout_secs` | 1 to 30, default 5. |

## What the model can and cannot do

- It supplies only the declared parameters. Anything else is rejected, never forwarded.
- It cannot choose the URL, host, headers, or credential.
- Placeholder values are percent-encoded, so a value cannot add path segments or query parameters.
- For `GET` and `DELETE` the other parameters go in the query string; for `POST` and `PUT` they go in a JSON body.
- With `confirm` on, the session loop refuses to run the tool until the model calls it again with `confirmed: true`
  after the user agreed (REQ-AST-008). The flag is not forwarded to the server.
- Responses are clipped to 1000 characters and are treated as data, not instructions.

## Limits

- Calls count toward the usual request caps because they ride on model turns (REQ-AST-011).
- `https://` works when the build has the default `tls` feature; otherwise use `http://` on a trusted network.
- No retries and no redirects: a failed call is reported to the model, which tells the user.
- Setting `confirm` to `false` is your choice for harmless actions (a light). Keep it on for anything that spends
  money, unlocks something, or cannot be undone.
