# R-05: Wyoming protocol and local intent matching

Date: 2026-11-07 · Feeds: ADR-0005, spec 0003, new fast-path requirement

## Wyoming ([OHF-Voice/wyoming](https://github.com/OHF-Voice/wyoming), MIT)

- **Wire format:** one JSON header line terminated by `\n`, optionally followed by a JSON data block and a binary payload (lengths declared in the header). "Basically JSONL + PCM audio". Peer-to-peer over TCP.
- **Audio:** `audio-start` / `audio-chunk` / `audio-stop`; chunks carry raw PCM with `rate`, `width`, `channels`, optional `timestamp`.
- **Services:** wake (`detect`, `detection`, `not-detected`), ASR (`transcribe`, `transcript`, plus streaming variants), TTS (`synthesize`, streaming variants), intent/handle, mic and snd.
- **Satellite:** `run-satellite`, `pause-satellite`, `satellite-connected`, `streaming-started/stopped`, `run-pipeline`.
- **Discovery:** `describe` → `info` lists services, models, languages (a capability handshake like ours).
- **No authentication or encryption**; meant for trusted networks.
- No reference client libraries on that page; trivial to implement (TCP + JSON).

## Hassil ([OHF-Voice/hassil](https://github.com/OHF-Voice/hassil), Apache-2.0)

Home Assistant's intent template language, implemented in Python (PyYAML only). Syntax: alternatives `(a|b)`, optional `[word]`, permutations `(a; b)`, slot lists `{name}`, reusable rules `<rule>`, numeric ranges `{0..100:brightness}`. Nothing in it requires a heavy engine; a Rust matcher for a subset is plausible but unmeasured.

## Home Assistant "Prefer handling commands locally" (community reports only)

Try local sentence matching first, then fall back to the LLM agent. User reports describe: LLM agents filtering some local intents, fallback quirks during continued conversations, and one report of a large latency regression with the option on (≈2.5 s → ≈52 s, attributed to the matching step; anecdotal). No official documentation on exact fallback order was found.

## Implications

1. **Wyoming is close to what ADR-0005 rejected on cost grounds, but its framing is simpler than ours.** WebSocket needs an HTTP upgrade, masking, and frame handling. Wyoming's JSONL + length-prefixed payload is easier on a tiny client and instantly compatible with the existing satellite ecosystem (Pi satellites, Voice PE-class devices).
2. Its gaps are exactly ours: **no auth/encryption** (conflicts with REQ-SEC-006) and **no concept of inbox announcements, indicator states, or follow-up policy**.
3. Local fast-path intents are a proven pattern, and its failure modes are known: matching must be **fast, bounded in time, and deterministic**, and must fall through cleanly to the LLM.

## Spec impact (recommended)

- **Revise ADR-0005 (Proposed amendment):** keep the satellite/core split, but define our protocol as a **Wyoming-compatible framing with a namespaced extension layer** (`clawdot.*` events, a `hello` carrying a token, optional TLS), rather than WebSocket. Benefits: smaller clients, near-free Wyoming bridge, and ecosystem reach. Cost: raw TCP needs our own TLS handling, and we track an external project's conventions. Decide in M0; spec 0003 open question #1 is updated accordingly.
- **New requirement (fast-path):** a bounded-time local matcher over a small hassil-style template set for stop/cancel/volume/timer/alarm/music-control, handled without an LLM call; fallthrough on no-match. Add REQ-AST-010; budget its memory in spec 0002; add latency and no-match fallthrough tests.
- Do not copy HA's behaviour of letting the LLM agent filter intents; our matcher is authoritative for its own grammar.

## Confidence
Wyoming: high (primary README). Hassil syntax: high. HA fallback behaviour: low (forum reports).
