# Recipe: calendar (read-only)

"What is on tomorrow?" without giving the assistant a way to change your calendar.

File: [`calendar-home-assistant.json`](../../examples/recipes/calendar-home-assistant.json). It reads one Home Assistant calendar, which can itself be Google Calendar, Microsoft 365, CalDAV or an ICS feed. Home Assistant expands recurring events, handles time zones and holds the OAuth refresh, which are the parts that make a direct calendar API awkward for a core that reads one fixed token at start-up.

The call is `GET /api/calendars/{entity_id}?start=...&end=...` with the long-lived token in `HA_TOKEN`. Home Assistant's REST documentation (read 2026-11-09) gives `start` and `end` as timestamps and does not show the response shape.

| Tool | Tier | Why |
|---|---|---|
| `calendar_events` | `read_only` | Reads only. The token should belong to a Home Assistant user who cannot do more than read the calendar; the tool has no way to create or delete events, and no such tool is offered. |

The model works out `start` and `end` from the `current_time` tool ("tomorrow" is a day in the speaker's time zone), so ask for a time zone offset in the description (the file does). Event titles and descriptions come from other people (a meeting invitation can carry any text), so the description says never to follow instructions in them, and the replay eval `rcp-calendar-text-cannot-delete-events` shows a title that orders deletions is read out as data and nothing else runs. A calendar invite asking for a pull request is also an existing eval (`inj-calendar-invite-asks-for-a-pull-request`).

## Setup

1. In Home Assistant, add the calendar integration you use and note the entity id (for example `calendar.family`). Edit the URL in the file.
2. Make a Home Assistant user for this, create its long-lived token, and `export HA_TOKEN=...` for the ClawDot service. If you already use the [Home Assistant recipe](home-assistant.md), use a separate, lower-privilege token.

## Not covered

An ICS-feed or CalDAV reader outside Home Assistant is not provided. Recurring events (`RRULE`) are the difficulty: a naive reader shows only the first occurrence. If you cannot run Home Assistant, a small sidecar built on a library that expands them, answering `GET /agenda?start=&end=` with a short JSON list, would fit the same tool shape; that is future work.

## Failure modes

| Symptom | Cause |
|---|---|
| `401` | Wrong token. |
| `404` | Wrong calendar entity id. |
| `400` | `start` or `end` is not a timestamp Home Assistant accepts; the model is told and retries once. |
| Empty answer | No events in the window, or the integration has not synchronised. |
| A cut-off list | Responses are clipped at 1,000 characters; a busy day is summarised, ask for a shorter window. |

## Verify

`scripts/cargo.sh test -p clawdot-core --test recipes` checks the URL, including that the `:` in a time is percent-encoded. `python3 examples/recipes/fake_server.py`, or the real instance (a read needs no `--dry-run`).

**Not verified:** the response shape and any live call.
