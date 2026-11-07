# Extensibility

Status: Draft. Decision record: [ADR-0010](../adr/0010-extension-model.md). Requirements: [spec 0001 §4.9](../specs/0001-product-requirements.md).

## Principle

**Bundle what the v1.0 definition needs and what is tiny. Everything else is data or a separate process. The core loads no third-party code.**

## Three rings

```mermaid
flowchart TB
    subgraph Core["Core (one static binary; counts against the budget)"]
        direction TB
        A[Wake, VAD, session, providers]
        B[Scheduler: timers, alarms, reminders]
        C[Fast-path matcher]
        D[Inbox + adapter engine]
        E[Media-source interface + stream player]
        F[MCP client + HTTP tools]
        G[Portal + API]
    end
    subgraph Packs["Packs (data, installed from the portal)"]
        P1[Inbox adapter mappings]
        P2[Fast-path grammar]
        P3[Tool presets: MCP, HTTP]
        P4[Prompt snippets]
    end
    subgraph Side["Sidecars and servers (separate processes, off-device)"]
        S1[Agent bridge]
        S2[Home Assistant integration]
        S3[Wi-Fi provisioner]
        S4[MCP servers]
        S5[Media backends, e.g. Spotify control]
    end
    Packs --> Core
    Core <--> Side
```

| Ring | What lives here | How it's added | Counts against budget? |
|---|---|---|---|
| **Core** | Wake word, STT/LLM/TTS clients, scheduler, fast-path, inbox with `claude-code` and `generic` adapters, portal, stream player, MCP client, HTTP tools | Part of the binary | Yes |
| **Packs** | Adapter mappings, grammar, tool presets, prompt snippets | Portal: paste a URL or pick from an index; hash-pinned. **User-installable packs ship in v1.x**; in v1.0 the format is used internally for the bundled adapters. | Marginally (data) |
| **Sidecars / servers** | Agent bridge, Home Assistant integration, Wi-Fi provisioner, MCP servers, heavy media backends | Docker, package, HACS, or another host | No (off-device) |

**Bundle rule:** bundle if it is essential to the v1.0 definition *and* small (roughly under 100 KB, no heavy dependencies). Otherwise it is a pack or a sidecar.

## Integration mechanisms

| You want to... | Use |
|---|---|
| Add a tool that calls an HTTP API (Home Assistant, a webhook) | **HTTP tool** (config or pack) |
| Add a rich or stateful tool set | **MCP server** (Streamable HTTP) |
| Support another agent's hook format (Codex, Gemini, Home Assistant events) | **Adapter mapping** (pack) |
| Add voice commands without an LLM call | **Fast-path grammar** (pack) |
| Add a music/audio source | **Media backend** (see below) |
| Talk to an agent that needs a long-lived process (Claude Code resume) | **Agent bridge** (sidecar, pulls replies over SSE) |
| Show state on a screen | Read semantic **display events** (satellite) or the `/display` page |

## Media-source interface

Music is agnostic by design. A backend declares **capabilities** (search, play, pause/resume, next/previous, queue, volume, now-playing, resume position) and one of two shapes:

- **Playback shape:** ClawDot receives an audio stream and plays it on its own speaker. Shipped in v1.0: internet radio and plain HTTP streams. Library servers (Subsonic-compatible, Plex, Jellyfin, Audiobookshelf) fit here by returning stream URLs.
- **Control shape:** ClawDot sends commands to a player elsewhere (Spotify Connect device, MPD, Music Assistant, a Home Assistant media player). Audio plays *there*, not on ClawDot's speaker. Spotify control via its Web API is this shape.

Playing Spotify *on ClawDot's own speaker* would need a Spotify Connect receiver on the device. That has budget and terms-of-service questions and is parked in [future-scope.md](../future-scope.md).

Resume positions (podcasts, audiobooks) are persisted by the core per item when the backend supports them.

## Example recipes

- **Home Assistant → ClawDot:** an HA automation POSTs to a `generic` inbox endpoint ("front door opened").
- **ClawDot → Home Assistant:** an HTTP tool forwarding the utterance to HA's conversation API (or an MCP connection to HA's MCP server). To be verified at M5.
- **Hermes as the brain:** point the LLM provider at Hermes's OpenAI-compatible gateway. Or Hermes → ClawDot through an HTTP call to an inbox endpoint. Verify against Hermes's own docs.
- **Claude Code:** native `http` hook to a `claude-code` endpoint; the bridge sidecar pulls replies over SSE and runs `claude -p --resume`.

## Trust model for packs and tools

1. Packs are data. No code executes on the device because of a pack.
2. A pack declares the tools, endpoints, and policies it adds; the portal shows this before install and requires explicit approval.
3. A pack cannot disable the `confirm` policy, the untrusted-text handling, or the usage limits.
4. HTTP tool URLs are admin-configured; the LLM supplies only schema-validated parameters.
5. Packs are pinned by hash and version. Updates require re-approval if requested permissions grow.
6. The core never fetches or runs anything from a pack at runtime beyond its declared data.

## What this explicitly excludes

In-process native plugins, scripting or WASM runtimes, and remote code download. Revisit only through a new ADR.
