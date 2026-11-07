# Configuration

ClawDot reads one human-editable JSON file with `--config FILE` (REQ-CFG-007). Every key is optional. Command-line
flags override the file. Unknown keys and impossible values stop startup with a message that names the key.

```json
{
  "data_dir": "/var/lib/clawdot",
  "listen": "0.0.0.0:8080",
  "public_url": "http://dot.local:8080",
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
| `listen` | HTTP address; bind to the LAN only (REQ-CFG-005) | `127.0.0.1:8080` |
| `public_url` | URL printed in integration snippets | none |
| `zone` | IANA time zone | UTC |
| `persona_file` | Replaces the default system prompt | built-in persona |
| `endpoints` | Inbox endpoints to create at startup (`NAME` or `NAME:ADAPTER`) | none |
| `llm.url`, `llm.model` | Language model; set together or not at all | none (fast path only) |
| `llm.fallback_model`, `llm.summary_model` | Optional; need `llm.url` and `llm.model` | none |
| `llm.key_env` | Name of the environment variable holding the key | `OPENROUTER_API_KEY` |
| `limits.max_requests_per_day` | Model requests per local day, at least 1 | 500 |
| `limits.daily_spend_usd` | Stop calling the model after this much spend | none |

## Rules

- **Secrets are never in the file.** The API key comes from the environment variable named by `llm.key_env`.
- **Validated at startup**, before anything is bound or written.
- **Hot reload** (a SHOULD in the spec) is not implemented: restart to apply changes. Rotating an endpoint token
  already works without a restart via the HTTP API.
