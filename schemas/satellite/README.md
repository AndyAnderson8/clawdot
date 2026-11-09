# Satellite protocol schemas

JSON Schema (draft 2020-12) for the header line of every control message in
[spec 0003](../../docs/specs/0003-satellite-protocol.md), protocol version 0.1 (REQ-SAT-012).

- [`frame-0.1.schema.json`](frame-0.1.schema.json): one file; pick the definition for a message by its `type`
  (for example `#/$defs/clawdot.hello`). Types that are not defined must be ignored by receivers (REQ-SAT-009), so
  the root schema is open.
- A message with fields has them inline in `data`. `audio-chunk` also carries `payload_length`; the binary payload
  itself is not JSON.
- `pattern` on `clawdot.play-url`'s `url` documents the `http(s)://`, no-whitespace rule. The Rust tests check that
  rule with the real parser rather than a regular-expression engine.

The schemas are kept in step with the code by `crates/protocol/tests/schema.rs`: every message the Rust types can
produce must validate, and the malformed examples there must be rejected by both the schema and the parser. When you
change the wire format, change the schema, the spec and that test together, and bump the version in the file name for
any breaking change.
