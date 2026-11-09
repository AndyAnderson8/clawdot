# Board file schema

JSON Schema (draft 2020-12) for the board files that describe one device to the portable device daemon, as defined in
[spec 0006](../../docs/specs/0006-device-layer-contract.md) (REQ-DEV-008). Design notes are in
[board-support](../../docs/architecture/board-support.md).

- [`board-1.schema.json`](board-1.schema.json): schema version 1. The `schema` key in a board file selects it. A breaking
  change makes a new file (`board-2.schema.json`); a loader refuses a version it does not know.
- Examples that must validate: [`boards/echo-dot2.json`](../../boards/echo-dot2.json),
  [`boards/desktop.json`](../../boards/desktop.json), [`boards/raspberry-pi.json`](../../boards/raspberry-pi.json).
- Every object is closed (`additionalProperties: false`): an unknown key stops startup and the message names it.
- Some rules are not expressible in the schema and are checked by the loader: a path has no `..` segment, `mic_channels` and
  `reference_channels` index channels that exist, `command` is present exactly when `kind` is `command`, and
  `leds.states` names only logical states the daemon knows.
- The file is data only. Program arguments are arrays of fixed text; the only placeholder anywhere is `{nn}` in an LED
  pattern name.

The daemon's loader (`clawdot-device`) validates with its own small checker rather than a schema library, to add no
dependency. A test keeps the two in step: it loads the shipped examples and a set of malformed files, and compares the
schema's property names with the loader's.
