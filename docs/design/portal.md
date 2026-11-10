# Portal design

Status: Accepted (design guidance; requirements are REQ-CFG-025..031, 060..070 in [spec 0001](../specs/0001-product-requirements.md)). The reasoning, palette and wireframes of this version are in [portal-v2.md](portal-v2.md); the design system choice is [ADR-0017](../adr/0017-portal-design-system-astryx.md); the stack is [ADR-0016](../adr/0016-portal-typescript-stack.md) and [ui-stack.md](ui-stack.md). The brand mark and logo rules are in [brand.md](brand.md).

## Who and what for

One person, mostly on a phone, who owns a small speaker running ClawDot. They open the page to see at a glance that the speaker is fine and what it is about to do (the Dashboard), and rarely to change how it behaves (Settings). Everything else is a place to look things up.

## Destinations

Four, with the Dashboard first. A top bar on a wide screen, a fixed bottom bar on a phone. The address fragment names the page (`#/dashboard`, `#/reminders`, `#/activity`, `#/settings`, and `#/settings/<section>`).

| Destination | What it is |
|---|---|
| Dashboard | The home page and the memorable element. One large sentence says what the speaker is doing (Ready, Listening, Thinking, Speaking, Muted, or why it cannot), with the ring from the logo beside it. Then: needs attention (language model not set, no speaker, no voice, clock not set, no password, muted, quiet hours on), each with a link to the fix; next up (the next five timers, alarms and reminders, with Cancel and a quick timer); recent (the latest actions and events); and in a side column "Say something" (test speak, disabled with the reason and a fix link when the device says it cannot speak) and today's usage. "Talk now" and "Mute" are shown disabled with the reason, because the page has no way to start those yet (REQ-CFG-031) |
| Reminders | The full list of timers, alarms and reminders with edit, cancel, repeat days and dates |
| Activity | What came in (events from inputs) and what the assistant did (the audit log), newest first, with filters and paging; read-only (REQ-CFG-060) |
| Settings | Everything that is configuration, in sections: General (device name, time zone, language, theme), Assistant (language model, key variable name, personality, voice, how far it may go), Inputs (create, copy, rotate and delete webhooks; snippets), Tools and connections (tools with risk tiers, tool servers, skills), Notes, Safety (usage and action limits, quiet hours, dry run), Security (password, sign out), About and diagnostics (version, release notes, diagnostics, recent problems, save and load a settings file, board) |

On a phone, Settings is a list of sections and each section opens as its own screen with a "Settings" back link; on a wide screen the list stays on the left. Each settings group is a ruled form with its own Save; under each field one line says where the value comes from and whether it applies right away (REQ-CFG-062). Saved values that are not yet running appear in a "Restart needed" block; there is no Restart button because the core cannot restart itself safely. Keys are entered as the name of an environment variable; the page never asks for, shows or stores a secret.

## Look

Light by default whatever the system prefers; dark only by explicit choice or "match system" (Settings, General, kept by this browser). System fonts only. Neutral palette from the Astryx neutral theme, plus one brand colour (Lagoon `#0b6e63`) used only for live and connected states. No cards, shadows or gradients; one filled area per screen (the status block). Controls are the platform's own: real `<select>`, checkbox, radio, date and time inputs, lightly styled, never custom popups. Tokens: `portal/src/tokens.css`. Screenshots: [dashboard, phone](screenshots/dashboard-phone.png), [dashboard, desktop](screenshots/dashboard-desktop.png), [settings, desktop](screenshots/settings-desktop.png), [inputs, phone](screenshots/settings-inputs-phone.png), [reminders, phone](screenshots/reminders-phone.png), [dark theme, phone](screenshots/settings-dark-phone.png), [sign-in](screenshots/sign-in-phone.png), [setup](screenshots/setup-phone.png).

## Principles

1. The dashboard is a sentence, not a set of widgets. State is told in words first; the ring only confirms it, and breathes slowly only while the speaker listens, thinks or speaks (not under reduced motion).
2. Colour means state. Everything else is ink on paper.
3. Space and ruled lists do the grouping. Surfaces are reserved for the status block, the restart notice and one-time secrets.
4. Words first. Plain sentence case, active voice; errors say what went wrong and how to fix it; empty states invite the next step. No product names as defaults and no personal examples; inputs are described as "events from anything that can send a web request", and coding-agent hooks are one adapter in a neutral list.
5. Never show a dead control. If something cannot work, say why beside it and link to the fix (REQ-CFG-031, REQ-CFG-070).
6. Destructive row actions (cancel, delete, forget) are red text buttons and ask first; secrets are shown once, then never again.
7. Quality floor: AA contrast in both themes, a 3 px focus ring, 44 px tap targets, keyboard operable (real links, buttons and form controls), focus moves to the page after a move, reduced motion respected, no horizontal scroll at 360 px (checked for every page in the browser test).
