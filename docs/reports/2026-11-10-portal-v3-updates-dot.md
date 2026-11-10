# 2026-11-10: portal redesign, recovery code removed, updates, and the Dot

What was done in one long session, what is proven and what is not, and what is waiting for the maintainer. "Proven" means checked by a test or by a run that is described here; "built" means the code exists and passes its tests against fakes.

## What changed

**Portal redesign (built, tested in a browser on Windows; CI not yet seen).**
- A dark sidebar (the speaker's colour) with a large logo that links home, and a wordmark with the light ring drawn as the "o". Light page by default.
- Five destinations: Home, Clock, Bluetooth, Features, Settings. Home holds the device card, what needs attention, numbers and a 14-day activity chart. Reminders, timers and alarms moved to Clock. Memory moved into Settings. Activity opens from Home.
- Settings are a tab strip (General, Assistant, Inputs, Tools, Memory, Safety, Security, About). Every pane uses one pattern: a card with a title, rows with the name and explanation on the left and the control on the right, one Save. Lists, forms, notices, help boxes, counts and dropdowns are one system, and a browser test now fails if a page puts loose blocks on the page or gives inputs and text areas different widths.
- The language model is chosen by service (OpenRouter, OpenAI, Anthropic, a model on the local network, or something else); each pane has "Reset to defaults" and About has "Reset all settings".
- "Test the speaker" moved from Home to Settings, Assistant. Skills are downloaded as a file and added from a file instead of two text boxes. About says what the project is, with a separate Version row.
- The stylesheet was rewritten as one sheet. The browser suite went from 28 to 34 flows and passes (34/34 locally); the unit suite is 50 tests.

**Recovery code removed (built, core tests pass).** No code is made, shown or asked for, in the core, the portal or the docs. A forgotten password is handled by holding the speaker's action button (a short reset window) or by `clawdot --reset-admin`. The default device name is now "ClawDot", never the machine's host name.

**Updates (built, tested against fakes; never run against a real release).** ClawDot checks for a newer release once a day, on demand, and tells the person; it installs only on confirmation and only a release signed with the project's Ed25519 key (REQ-UPD-001 to 011, [guide](../guides/updates.md), [ADR-0019](../adr/0019-updates-and-remote-access.md)). 13 unit tests, an API test and 3 browser flows cover it. Against the real release list it answers 404 because there is no release yet, and says so in plain words.

**Process.** The streamlined checks (path-scoped, with a nightly browser job) landed on `dev`. A temporary stabilization landing mode landed on `dev` (ADR-0018 on `dev`); it skips the CI wait for landings into `dev` until `scripts/gate-mode` is set back to `full`.

## On the Dot

- The new build (portal, core, updater) was copied to the Dot over the USB console, verified by SHA-256, and checked with `--version` before it replaced the old one. The previous program is kept as `clawdot.prev`.
- The supervisor script `/data/clawdot/run.sh` now sets `CLAWDOT_SUPERVISED`, restores `clawdot.prev` if the new program crashes within 30 seconds three times in a row, and keeps the firewall rule for the portal port.
- **Autostart is proven:** the Dot was rebooted over the console and ClawDot was serving the portal again 31 seconds later with nobody touching it. Its first connection to the speaker service was refused because that service was not up yet, and it reconnected on its own.
- Not re-tested after the update: a spoken question end to end. The speech servers on the maintainer's PC are running (port 8082), but a person has to say "Alexa" to hear it.
- The portal on the Dot is still in first-run setup, waiting for the maintainer to choose a password.

## Decisions made (judgement calls, [ADR-0019](../adr/0019-updates-and-remote-access.md), Proposed)

- **Updates:** check daily and on demand, confirm to install, signed releases only; with no key it only announces.
- **SSH:** worth having for a device with no screen, but off by default, key-only, user `clawdot` where the system allows, local network only, switched on from the signed-in portal. Whether the Dot image can run it is a B0 question.
- **USB file access:** no. It exposes the data directory with its keys and risks corrupting the storage the speaker writes to.
- **SMB:** no. A large service for a small device, sharing the same secrets. Settings export, diagnostics, skills files, signed updates and SSH cover the real needs.

## Needs the maintainer

1. Set the portal password on the Dot and say "Alexa" to hear a reply (latency was about 7 seconds before).
2. Create the update signing key ([guide](../guides/updates.md)) and decide whether releases will live on GitHub, so the update button can install. Until then it announces only.
3. The GitHub ruleset on `dev` may reject direct pushes now that landings skip CI: relax it as `docs/process/repo-settings.md` says, or return `scripts/gate-mode` to `full`.
4. Whether to rewrite history before the first release (personal strings are still in old commits).
5. Anything in the [open questions](../open-questions.md) added today.

## Left alone on purpose

Rules ("do X when Y") and background tasks need a product design. Fuzzing in CI needs a decision on CI time and a nightly toolchain. A software bill of materials needs a tool choice. A local-network name (`clawdot.local`) needs a dependency or a hand-written responder. Offline degraded mode needs a design. None has a single obvious approach, so none was started.
