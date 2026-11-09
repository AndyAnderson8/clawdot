# Portal design

Status: Accepted (design guidance; requirements are REQ-CFG-025..031 in [spec 0001](../specs/0001-product-requirements.md)). Builds on [brand.md](brand.md) for colour tokens and the logo, and on [ui-stack.md](ui-stack.md) (plain HTML, CSS, and JavaScript for now).

## Who and what for

One person on a phone, just after plugging in a small crab-themed speaker. The portal has to answer three questions fast: is it alive, what will it do for me, and how do I change that. The earlier single-page layout failed because it used internal words (inbox, scheduled, endpoints), hid tools and connections entirely, and showed radio stations nobody asked for.

## Palette

Derived from the logo (aqua ring on a lagoon puck, coral crab). Full tokens and contrast ratios are in [brand.md](brand.md).

| Name | Light | Dark | Used for |
|---|---|---|---|
| Foam | `#f4f8f7` | `#0b1a1f` | Page background |
| Lagoon ink | `#0e2a33` | `#e8f3f1` | Text, and the header band in both modes |
| Puck blue | `#1f4d5b` | `#24414a` | Lines and raised parts of the header |
| Ring aqua | `#2ee6c5` | `#2ee6c5` | The ring: active tab, "ready" state, accent in dark mode |
| Deep teal | `#0b6e63` | n/a | Accent for text and buttons on light pages (6.1:1 on white) |
| Shell coral | `#f2644f` | `#f2644f` | "Needs attention" marks only; never text on a light page |

## Type

System fonts only. One family (the system UI sans) with weight doing the hierarchy: 700 for tab and section titles, 600 for actions, 400 for text. Times use tabular numerals; URLs, tokens, and snippets use the system monospace. Scale ratio 1.25 (0.875 to 1.953 rem). Everything is left-aligned; paragraphs stay under 60 characters wide.

## Layout

```
+----------------------------------+
| (crab)  Kitchen Dot       (o) Ready |   header, always visible:
+----------------------------------+      logo, device name, ring = status
| Home  Reminders  Agents  Tools  > |   tab strip, scrolls sideways on a phone;
|  ~~~~                              |   the active tab sits under a ring arc
+----------------------------------+
|  Ready. One speaker is connected. |
|  Setup checklist  (o) (o) ( ) ( ) |   one column, ruled lists, actions
|  Say something to the speaker     |   on the row they belong to
+----------------------------------+
| ClawDot 0.1.0   What is new       |   footer: running version and release notes
+----------------------------------+
```

On a wide screen the same single column is kept (max 44 rem) so that nothing moves between phone and desktop.

## Tabs and the words used

| Tab | Replaces | Why this name |
|---|---|---|
| Home | Status | The first thing to see: is it working, plus the setup checklist and the test-speak box |
| Reminders | "Scheduled" | Covers alarms, timers, and reminders, which are the three things people already say out loud |
| Agents | Inbox | What arrives here is news from your coding agents (Claude Code and others); "Agents" says whose news it is |
| Tools | n/a | Everything the assistant can do besides talking |
| Connections | n/a | Outside tool servers (MCP). The page explains MCP in a sentence the first time |
| Skills | n/a | Bundles of instructions for the assistant (slice 2) |
| Webhooks | Endpoints | A webhook is the web address an agent posts to; this tab creates them and gives copy-paste setup |
| Settings | Admin password | Time zone, language model, voice, addresses, limits, password, and About with the version |

## Principles

1. The ring is the one memorable element. It appears as the status indicator (aqua ready, coral needs attention, grey starting) and as the arc under the active tab, echoing the light on the puck. Nothing else is decorated.
2. Lists with row actions, not stacks of cards. Surfaces are reserved for forms and for one-time secrets.
3. Plain sentence case. Errors say what went wrong and how to fix it; empty states invite the next step ("No reminders yet. Add one, or say 'remind me at 5'").
4. Never show a dead control. If the backend does not exist, say what the feature will do and that it is coming (REQ-CFG-031).
5. Destructive actions use an outlined button and a confirm step. Secrets are shown once, then never again.
6. Quality floor: AA contrast, a visible 3 px focus ring, reduced motion respected, 44 px tap targets, keyboard-operable tabs.

## Changes made in review of the first plan

The first sketch was a tab strip with boxed cards, a big status number, and an underline for the active tab. It read like any admin template. It became ruled lists, a sentence for status instead of a number, and the ring arc for the active tab and status.
