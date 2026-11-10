# Portal v2: design plan

Status: Proposed (supersedes the layout, tabs and palette parts of [portal.md](portal.md) once built; requirements stay REQ-CFG-025..031 and 060..070). The design-system choice is in [ADR-0017](../adr/0017-portal-design-system-astryx.md).

## Why a second pass

The maintainer's verdict on v1: the front end was badly designed. Specifically: dropdowns that did not look native, a Home page that was a setup checklist instead of a dashboard, ten tabs where one Settings area should hold the configuration, an Agents tab (and "Claude Code" as the default example everywhere) that made one use of the product look like its purpose, dark mode by default, and a look that read as generated. This plan answers each point.

## Who and what for

One person, mostly on a phone, who owns a small speaker that runs ClawDot. They open the page for two reasons: to see at a glance that the speaker is fine (and what it is about to do), and, rarely, to change how it behaves. The first reason is the home page and has to be a glance. The second is Settings and has to be complete. Everything in between (history, reminders) is a place to look things up.

## Palette

Light by default, whatever the operating system prefers. Neutral values are Astryx's neutral theme (MIT) so the page shares its token contract (ADR-0017); one colour is ours.

| Name | Hex | Role |
|---|---|---|
| Paper | `#ffffff` | Page and form surfaces |
| Ash | `#f1f1f1` | The one filled area per screen (the status block) and disabled fills |
| Ink | `#1b1b1b` | Text, primary buttons, focus ring |
| Graphite | `#474747` | Secondary text (9.3:1 on Paper) |
| Hairline | `#d4d4d4` | Rules between rows (decoration only); control edges use `#8a8a8a` (3.45:1) |
| Lagoon | `#0b6e63` | The only brand colour: the live-state ring, "connected" and "ready" marks (6.1:1 on Paper) |

Signals borrowed from the same theme, used for meaning only and never as decoration: error text `#c0262c` (5.9:1), attention text `#836700` (5.4:1 on Paper; never placed on its tint), success text `#2f7d33` (5.1:1). Dark theme (explicit choice or "match system"): Ink and Paper swap around `#1b1b1b` / `#f1f1f1`, Graphite becomes `#b3b3b3` (8.2:1), Lagoon becomes `#4fb1a0` (6.7:1), signals become their lighter Astryx dark values (all above 9:1 on `#1b1b1b`).

## Type

System fonts only: the page may not load a font (`default-src 'none'`) and a font file would cost more than the rest of the page. One family, so hierarchy comes from size, weight and space.

| Role | Size and weight | Used for |
|---|---|---|
| Status line | 2rem (1.75rem on a phone), 600, tight leading, tracking -0.02em | The one sentence that says what the speaker is doing |
| Page title | 1.5rem, 600 | Dashboard, Reminders, Activity, Settings, and a settings section |
| Section title | 1.0625rem, 600 | "Needs attention", "Next up", a settings group |
| Body | 1rem, 400, 1.5 | Everything else; lines stay under 62 characters |
| Supporting | 0.875rem, 400, Graphite | Hints, times, sources |
| Code | system monospace, 0.875rem | Addresses, tokens, snippets only (never labels) |

Numerals are tabular wherever times and counts line up. Everything is left-aligned. No capital-letter labels, no eyebrows.

## Layout concept

The Dashboard is the memorable element. It is not a grid of tiles. It is one quiet page read top to bottom: a large sentence that says what the speaker is doing, with the ring from the logo beside it as the live indicator (a still ring when idle; a slow breath while it listens or speaks; nothing moves under reduced motion), then what needs attention, then what is next, then what just happened. Sections are separated by space and one hairline, not boxed. Only the status block has a filled background (Ash).

Four destinations: Dashboard, Reminders, Activity, Settings. A top bar holds the device name and mark on the left and the four links on the right. On a phone the same four links form a fixed bottom bar (thumb reach), and the top bar keeps only the name and mark.

Desktop (about 1100 px; content column 44rem, side column 18rem):

```
+------------------------------------------------------------------------------+
| (mark) Living room speaker          Dashboard  Reminders  Activity  Settings |
+------------------------------------------------------------------------------+
|                                                                              |
|  +-----------------------------------------------+   Do now                  |
|  | (o)  Listening.                               |   [ Say something ]       |
|  |      Last heard 4 min ago: "what time is it"  |   [ Talk now      ]       |
|  +-----------------------------------------------+   [ Mute          ]       |
|                                                                              |
|  Needs attention                                    Today                    |
|  ----------------------------------------------   12 of 200 requests         |
|  Clock is not set   Reminders wait for it.  Fix     $0.04 of $1.00            |
|  No password        Anyone on this network. Set it                           |
|  ----------------------------------------------                              |
|                                                                              |
|  Next up                                          Add timer, alarm, reminder  |
|  ----------------------------------------------                              |
|  Pasta           Timer      in 8 min                          Cancel         |
|  Wake up         Alarm      Tomorrow at 7:00 am               Cancel         |
|                                                                              |
|  Recent                                              See all activity         |
|  ----------------------------------------------                              |
|  2:14 pm  Set a timer for 10 minutes                                          |
|  1:50 pm  Build finished (from "ci")                                          |
+------------------------------------------------------------------------------+
```

Phone (360 px; one column; bottom bar):

```
+------------------------------+
| (mark) Living room speaker   |
+------------------------------+
| +--------------------------+ |
| | (o) Listening.           | |
| |     Last heard 4 min ago | |
| +--------------------------+ |
| [Say it] [Talk now] [Mute]   |
|                              |
| Needs attention              |
| -------------------------    |
| Clock is not set        Fix  |
| No password             Set  |
|                              |
| Next up              Add     |
| -------------------------    |
| Pasta   in 8 min      Cancel |
| ...                          |
+------------------------------+
| Dash | Remind | Activity | Settings |   (fixed bottom bar)
+------------------------------+
```

Settings on desktop is two columns (section list on the left, one section on the right); on a phone the list is its own screen and each section opens as a screen with a "Settings" back link (`#/settings/assistant`):

```
Desktop                                   Phone
+-------------+----------------------+    +--------------------+   +--------------------+
| General     | Assistant            |    | Settings           |   | < Settings         |
| Assistant * | Model   [native sel] |    | General          > |   | Assistant          |
| Inputs      | Key variable [____]  |    | Assistant        > |   | Model  [native sel]|
| Tools       | Persona  [textarea]  |    | Inputs           > |   | ...                |
| Notes       | Voice    [native sel]|    | Tools            > |   | [Save assistant]   |
| Safety      | [Save assistant]     |    | Notes            > |   +--------------------+
| Security    |                      |    | Safety           > |
| About       |                      |    | Security         > |
+-------------+----------------------+    | About            > |
                                          +--------------------+
```

Alignment: left-aligned text and labels above controls; actions right-aligned on the row they belong to; the status sentence is never centred.

## Settings map (everything configurable, nothing lost)

| Section | Holds |
|---|---|
| General | Device name, time zone, language, theme (Light, Dark, Match system; Light by default) |
| Assistant | Model address, model, backup model, summary model, key variable name, persona, voice, and the "how far it may go" limits (rounds, tool calls, seconds, say the plan, say progress) |
| Inputs | Webhooks and event feeds: create, copy address, new token, delete, snippets (curl, PowerShell, GitHub Actions, Home Assistant, and a neutral list of adapters in which coding agents are one item) |
| Tools and connections | Tools with risk tiers and on/off, tool servers, skills (add, edit, import, export) |
| Notes | The notes the assistant keeps; add, forget, forget all |
| Safety | Limits per day, quiet hours, daily spend, dry run |
| Security | Admin password, sign out |
| About and diagnostics | Version, build, release notes, diagnostics file, recent problems, save and load a settings file, addresses, board |

## Principles

1. The dashboard is a sentence, not a set of widgets. State is told in words first; the ring only confirms it.
2. Colour means state. Only Lagoon (live, connected) and the three signals carry colour; everything else is ink on paper.
3. Space and rules do the grouping. One filled area per screen. No card stacks, no shadows, no gradients.
4. Native controls. Selects, checkboxes, radios, date and time inputs are the platform's own elements, lightly styled, so they behave on every phone the way people expect.
5. Words first. Sentence case, active voice, buttons say what they do, errors say what went wrong and how to fix it, empty states invite the first action. No brand or product names as defaults; examples are generic ("Pasta", "Back door", "Build finished").
6. Never a dead control. If something cannot work, it is disabled with the reason beside it and a link to the fix.
7. Quality floor: AA contrast in both themes, a 3 px focus ring, 44 px tap targets, keyboard operable, reduced motion respected, no horizontal scroll at 360 px.

## Review against the brief and against the generated-page tells

I checked the first draft against the brief and against the tells that generated pages share (cream and terracotta; dark with one acid accent; broadsheet rules; the card kit with one radius, soft shadows and gradients; template chrome such as capital-letter eyebrows, middle-dot strings and monospace labels).

| Finding | Change |
|---|---|
| First draft put status, attention, next and recent in four equal bordered cards. That is the card kit. | One filled block for the status, everything else is ruled lists on the page. |
| First draft used the teal brand colour for buttons, links, tabs and badges. Accent everywhere is another tell, and the logo ring stopped meaning anything. | Buttons are ink; Lagoon appears only for live and connected states. |
| First draft used a left sidebar of icons plus a top search box, the admin-template default. | Four text links in a top bar, bottom bar on a phone. No icons in navigation. |
| First draft had a small uppercase "STATUS" label over the big sentence. | Removed; the sentence is its own label. No capital-letter labels anywhere. |
| First draft used pill radii on everything. | Two radii with reasons: 6 px on controls and the status block, 0 on rules; only the ring is round. Buttons are not pills. |
| Dark palette would have been the maintainer's first impression. | Light is the default regardless of the OS; dark is an explicit setting. |
| Copy used "agent", "webhook", "endpoint" as headings. | "Inputs: events from anything that can send a web request". Technical words appear only in the snippets. |

What remains deliberately conventional: system fonts, a top bar and a bottom bar, native controls. Those are the point, not a gap in imagination: this is a utility a person opens for ten seconds.
