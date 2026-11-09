# Make the speaker say something (announce)

A script, a home-automation rule or a bridge can make ClawDot speak a message, with no coding agent involved. This is the one-way "say this" path: `POST /api/announce`, and the `clawdot say` command on top of it. Requirements: REQ-INB-021 to REQ-INB-027 ([spec 0001](../specs/0001-product-requirements.md)).

## What it is, and what it is not

An announcement is an ordinary inbox event on one of your **endpoints**. It does not go straight to the speaker:

* the endpoint's **mute**, **minimum priority**, **quiet hours** and **burst limit** apply, so a flood or a message at 3 a.m. is handled like any other event (only `urgent` is spoken in quiet hours);
* it waits while the speaker is busy and is spoken when it is idle, with the source named first: "From Kitchen: Dinner is ready.";
* its words are **data**. A short message (200 characters or fewer) is cleaned for speech (markup and links removed, control characters dropped) and spoken as it is. A longer one goes to the summary model, which has no tools and sees the text inside a data fence. Nothing in the text can start a tool, set a timer, write a note or answer a question the assistant is holding for your "yes" (REQ-INB-022). The evals `inj-announce-text-cannot-run-a-tool` and `inj-announce-text-cannot-say-yes` check this;
* it is written to the **audit log** (`GET /api/audit`, tool `announce`, trigger `announce:<source>`) without the token, and it does not count against the daily action caps.

It is not a way to talk to the assistant: the text is never an instruction to it. To drive a coding agent by voice use a hook endpoint ([REQ-INB-003](../specs/0001-product-requirements.md)).

## Set it up

1. Create an endpoint for the sender, using the `generic` adapter (the adapter is not used by announce, but the endpoint carries the token, the filter and quiet hours):
   ```
   clawdot --create-endpoint house:generic
   ```
   The token (`cdt_...`) is printed once. Put it in an environment variable or a secrets file on the sending machine, never in a script you commit. One endpoint per sender lets you mute or revoke one without the others.
2. Test it from that machine:
   ```
   export CLAWDOT_URL=http://clawdot.local:3335
   export CLAWDOT_TOKEN=cdt_...
   clawdot say "Dinner is ready" --source Kitchen
   ```

## The request

`POST /api/announce` with `Authorization: Bearer <endpoint token>` (or `X-ClawDot-Token`) and a JSON body:

| Field | Meaning |
|---|---|
| `text` | Required. What to say, up to 2,000 characters. |
| `title` | Optional headline, up to 200 characters. Default: the first sentence of `text`. |
| `source` | Optional name spoken as "From <source>", up to 60 characters. Default: the endpoint's name. |
| `priority` | `low`, `normal` (default) or `urgent`. Only `urgent` is spoken during quiet hours. |
| `reply` | `true` lets the listener answer it. Default `false`: a spoken "tell it ..." gets "nothing to reply to". |
| `ttl_secs` | 1 to 86,400. If it has not been spoken by then it is dropped, so "the oven is hot" is never read an hour late. Default: no limit. |
| `endpoint` | Only with an administrator session (instead of an endpoint token): the endpoint to speak on. |

Answers: `202 {"queued":true,"id":7}`; `202 {"queued":false,"filtered":"muted"}` when the endpoint's filter dropped it (also `below_priority`); `400` naming the field that is wrong; `401` for a missing or wrong credential; `429` when the endpoint's burst limit or the failed-attempt limit is reached; `413` for a body over 32 KB. The text is never echoed back.

## Snippets

Each keeps the token in an environment variable. `$CLAWDOT_URL` is, for example, `http://clawdot.local:3335`.

**curl**

```sh
curl -sS -X POST "$CLAWDOT_URL/api/announce" \
  -H "Authorization: Bearer $CLAWDOT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"text":"The backup finished","source":"NAS","priority":"low","ttl_secs":3600}'
```

For text that may contain quotes, build the JSON with a tool instead of by hand: `jq -n --arg t "$MSG" '{text:$t,source:"NAS"}' | curl -sS -X POST ... -d @-`.

**PowerShell**

```powershell
$body = @{ text = "The build is green"; source = "Build server"; priority = "normal" } | ConvertTo-Json
Invoke-RestMethod -Method Post -Uri "$env:CLAWDOT_URL/api/announce" `
  -Headers @{ Authorization = "Bearer $env:CLAWDOT_TOKEN" } `
  -ContentType "application/json" -Body $body
```

**clawdot say** (the same binary as the server; it needs no configuration file)

```sh
clawdot say "Dinner is ready" --source Kitchen
clawdot say "The garage door is open" --priority urgent --ttl 600
clawdot say "Open the front door?" --reply
```

Options: `--source`, `--title`, `--priority`, `--reply`, `--ttl SECS`, `--endpoint NAME` (administrator session only), `--url` (default `$CLAWDOT_URL`, else `http://127.0.0.1:3335`) and `--token-env VAR` (the **name** of the variable that holds the token, default `CLAWDOT_TOKEN`). There is no flag for the token itself, so it never appears in a process list or shell history. It prints `queued` and exits 0, or prints one line to stderr and exits non-zero (2 for a bad command line, 1 for a refusal or no connection).

**Home Assistant** (`configuration.yaml`; keep the whole header value in `secrets.yaml` as `clawdot_bearer: "Bearer cdt_..."`)

```yaml
rest_command:
  clawdot_say:
    url: "http://clawdot.local:3335/api/announce"
    method: post
    headers:
      authorization: !secret clawdot_bearer
    content_type: "application/json"
    timeout: 10
    payload: >-
      {{ {"text": message, "source": source | default("Home Assistant"),
          "priority": priority | default("normal"), "ttl_secs": ttl | default(900)} | to_json }}
```

Call it from an automation:

```yaml
action:
  - action: rest_command.clawdot_say
    data:
      message: "The washing machine has finished"
      source: "Laundry"
      priority: "low"
```

`to_json` escapes the message, so quotes in a template cannot break the request. These snippets follow the documented Home Assistant `rest_command` options; they have not been run against a live Home Assistant here ([open-questions.md](../open-questions.md)).

**GitHub Actions** (a step that speaks when a job fails; `CLAWDOT_URL` must be reachable from the runner, for example over a tunnel you control)

```yaml
- name: Say it
  if: failure()
  run: |
    jq -n --arg t "Build failed on ${GITHUB_REF_NAME}" '{text:$t,source:"GitHub",priority:"urgent",ttl_secs:1800}' |
      curl -fsS -X POST "$CLAWDOT_URL/api/announce" -H "Authorization: Bearer $CLAWDOT_TOKEN" \
        -H "Content-Type: application/json" -d @-
  env:
    CLAWDOT_URL: ${{ secrets.CLAWDOT_URL }}
    CLAWDOT_TOKEN: ${{ secrets.CLAWDOT_TOKEN }}
```

**ntfy and Discord** do not need hand-written scripts: the [bridges](../../bridges/README.md) subscribe to a topic or channel and post here, with filters, rate limits and cleaned text.

## Failure modes

| Symptom | Cause |
|---|---|
| `401` | The token is wrong, was rotated, or the endpoint is switched off. Create or rotate it with `clawdot --rotate-endpoint NAME`. |
| `{"queued":false,"filtered":"muted"}` | The endpoint is muted or its minimum priority is above the message's. Not an error: the filter did its job. |
| Accepted, but nothing is heard | The speaker is busy or muted (the message waits), it is quiet hours and the priority is not `urgent`, or `ttl_secs` ran out first. |
| `429` | More than the burst limit (20, then 5 a second) from one endpoint. Slow down or coalesce messages at the sender. |
| The speaker says only a headline | No language model is configured and the text is over 200 characters: the fallback speaks the title, never the long text (REQ-INB-018). |

## Security notes

* The announce token lets its holder make the speaker say things. Treat it like a door key: one endpoint per sender, rotate on a leak, and keep it out of repositories.
* The speaker names the source first, so an announcement cannot pass itself off as the assistant, but a sender chooses its own `source` word. Do not give a token to anything you do not trust to pick a truthful name.
* Spoken text can still be rude or misleading. It cannot do anything else.
