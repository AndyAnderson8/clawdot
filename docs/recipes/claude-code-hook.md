# Recipe: Claude Code hook (the inbox)

Claude Code tells the speaker when it finishes or needs you, and you can answer by voice. This is an inbox endpoint, not a tool: nothing the hook sends can run anything (REQ-INB-010). Requirements: REQ-INB-003, REQ-INB-013, REQ-INB-015.

Risk tier: not applicable to the hook itself (it creates events, not tool calls). A spoken `instruct` reply back to Claude Code is `costly`: the host reads your exact words back and sends them only after a yes (REQ-SEC-011).

## Setup

1. On the ClawDot device create the endpoint and keep the token it prints once:
   ```
   clawdot --create-endpoint claude-code:claude-code
   ```
2. On the computer where Claude Code runs, put the token in an environment variable (a shell profile or the system environment, not a file you commit):
   ```
   export CLAWDOT_TOKEN=cdt_...
   ```
3. Add the hooks to `~/.claude/settings.json` (or a project's `.claude/settings.json`). Claude Code's HTTP hook takes `url`, `headers`, `allowedEnvVars` and `timeout`; a header may use `$VAR`, and **only variables listed in `allowedEnvVars` are filled in** (Claude Code hooks documentation, read 2026-11-09):

```json
{
  "hooks": {
    "Stop": [
      { "hooks": [ {
          "type": "http",
          "url": "http://clawdot.local:3335/hook/claude-code",
          "headers": { "Authorization": "Bearer $CLAWDOT_TOKEN" },
          "allowedEnvVars": ["CLAWDOT_TOKEN"],
          "timeout": 5
      } ] }
    ],
    "Notification": [
      { "hooks": [ {
          "type": "http",
          "url": "http://clawdot.local:3335/hook/claude-code",
          "headers": { "Authorization": "Bearer $CLAWDOT_TOKEN" },
          "allowedEnvVars": ["CLAWDOT_TOKEN"],
          "timeout": 5
      } ] }
    ]
  }
}
```

The portal's Webhooks tab generates the same snippet with the token filled in; this version keeps the token out of the file.

4. To answer by voice ("tell it to go ahead"), run the reference bridge on the same computer: `CLAWDOT_TOKEN=... clawdot-bridge --url http://clawdot.local:3335 --endpoint claude-code`.

## Why a slow or offline speaker cannot stall Claude Code

The HTTP hook has no `async` option (that belongs to command hooks), so the protection is the settings that are there: a failed, refused or timed-out request is a **non-blocking error and Claude Code carries on**, unless you set `onFailure` to `block` (do not). The 5-second `timeout` caps how long a dead speaker can hold up the end of a turn; the default is 600 seconds, so do not leave it out. ClawDot answers `202` with an empty `{}` body, which Claude Code reads as "no decision" (a 2xx with an empty body is success). Details of the response rules are in the hooks documentation; the `Stop` event carries `last_assistant_message`, which ClawDot's adapter uses as the event text.

## What gets spoken

"From Claude Code: <project> finished. <two-sentence summary>. Want details?" The text is data; it is summarised by the summary model, which has no tools, and bounded (REQ-INB-018). Bursts of `Stop` events from one session within 30 seconds are merged into one announcement.

## Failure modes

| Symptom | Cause |
|---|---|
| Nothing is spoken, no error in Claude Code | The variable is not in the environment Claude Code started with, or is not listed in `allowedEnvVars`: the header becomes `Bearer ` with an empty token and ClawDot answers `401` (a non-blocking error). Check with `curl` using the same token. |
| `401` | Token rotated (`clawdot --rotate-endpoint claude-code`) or endpoint switched off. |
| `422` | The `claude-code` adapter has no rule for that hook event (only the events it maps are spoken). An endpoint created with the `generic` adapter accepts anything but speaks it as a plain notification, so create it as `claude-code:claude-code`. |
| `429` | More than 20 events in a burst. Normal `Stop` traffic never reaches it. |
| Spoken at night | Set quiet hours on the endpoint; only urgent events pass. |
| Works on the LAN, not away from home | The speaker is not reachable from that network. Use a VPN you run; do not expose the port to the internet. |

## Verify

* `scripts/cargo.sh test -p clawdot-core` covers the adapter on a real Stop payload, the 202/401/422/429 statuses and coalescing; the replay evals `inj-event-text-instructs-an-action` and `uc2-event-turn-proposes-only` show event text cannot act.
* By hand, with no Claude Code: `curl -X POST http://clawdot.local:3335/hook/claude-code -H "Authorization: Bearer $CLAWDOT_TOKEN" -d '{"hook_event_name":"Stop","session_id":"t","cwd":"/work/demo","last_assistant_message":"Done. All tests pass."}'`.

**Not verified:** a live Claude Code session against this exact file (the field names and rules come from the hooks documentation only), the `Stop`/`Notification` payloads beyond the fields named above, and Claude Code versions before the `allowedEnvVars` field existed.
