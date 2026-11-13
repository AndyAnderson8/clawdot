# The ClawDot HTTP API

[openapi.yaml](openapi.yaml) describes, in OpenAPI 3.1, the JSON HTTP API the core serves: the calls the web
portal makes under `/api`, and the public endpoints other programs post to (`/healthz`, `/hook/{endpoint}`,
`/api/announce`, `/bridge/{endpoint}/replies`).

## What it is for

Third-party clients, extra screens (a wall tablet, a phone shortcut), and scripts. Anything the portal can do,
a script can do with the same calls. Load the file into any OpenAPI viewer or code generator.

## Conventions

- Bodies and answers are JSON. Failures are `{"error": "<plain sentence>"}` with a matching status code.
- Everything under `/api` except `auth`, `setup`, `login`, `logout` and `recover/window` needs
  `Authorization: Bearer <session token>`. In open mode (chosen at first setup, private network only) a
  private-network peer needs no token.
- Webhook endpoints use an endpoint token instead (`Authorization: Bearer` or `X-ClawDot-Token`). `POST
  /api/announce` accepts either an endpoint token or an administrator session.
- No secrets come back. Tokens are returned once, when made or rotated. The language-model key pasted into the
  portal is write-only: the API returns a masked hint, never the key. Text from agents, notes and tools is
  untrusted; show it as text.

## How it is kept in sync

The file is written by hand from the source (`crates/core/src/ui.rs`, `http.rs`, `announce_api.rs`) and the
response types in `portal/src/types.ts`. The test `crates/core/tests/openapi.rs` runs in `scripts/check.sh` and
fails when the set of `METHOD /path` pairs in the code differs from the set in the file, in either direction.
It does not check request or response shapes, so when you change a body or an answer, change the file in the
same branch. Add a route in the code, and the test tells you which operation is missing.

## Example session

```sh
CORE=http://clawdot.local:8080

# Sign in (password mode); keep the token.
TOKEN=$(curl -s -X POST "$CORE/api/login" \
  -H 'Content-Type: application/json' \
  -d '{"password":"your password"}' | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')

# Read what the assistant is doing.
curl -s "$CORE/api/status" -H "Authorization: Bearer $TOKEN"

# Make the speaker say a phrase, optionally in a chosen voice (see GET /api/voices).
curl -s -X POST "$CORE/api/speak" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"text":"Hello from a script","voice":"nova"}'
# 202 {"ok":true,"queued":true}, or e.g. 409 {"ok":false,"reason":"no_speaker","message":"..."}

# Sign out.
curl -s -X POST "$CORE/api/logout" -H "Authorization: Bearer $TOKEN"
```

A script that only needs to say something can use a webhook endpoint token instead of a session:

```sh
curl -s -X POST "$CORE/api/announce" \
  -H "Authorization: Bearer $ENDPOINT_TOKEN" -H 'Content-Type: application/json' \
  -d '{"text":"The backup finished","priority":"low"}'
```
