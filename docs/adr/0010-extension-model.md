# ADR-0010: Extension model: no loadable code; MCP, HTTP tools, packs, sidecars

- **Status:** Proposed (maintainer review)
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

ClawDot must be easy to integrate with anything (Spotify, Home Assistant, Hermes, other agents, future services) without us building every integration, while staying within the resource budget ([spec 0002](../specs/0002-resource-budget.md)) and keeping a device that holds a microphone and API keys safe. The maintainer wants integrations installable and manageable from the device's web portal.

## Decision drivers

- Budget: a plugin runtime or loaded native code costs RAM and binary size.
- Security: third-party code on an always-listening device with secrets is a high-risk surface.
- Maintainability: a one-person project cannot support an in-process plugin ABI.
- Ease of integration: most integrations are "call an HTTP API" or "speak MCP".
- Portal-installable: installing should be as simple as pasting a URL.

## Options considered

1. **In-process native plugins (dynamic libraries).** Fast and flexible; unsafe, ABI-fragile, and un-sandboxed.
2. **Embedded scripting or WASM runtime.** Sandboxable, but adds runtime size and RAM, a new API to design and support, and a larger attack surface.
3. **Out-of-process extensions only (MCP, HTTP tools, sidecars) plus declarative data packs.** No code runs in the core on behalf of third parties.

## Decision

Option 3. The core loads **no third-party code**. Extension happens in four layers, in order of preference:

1. **MCP servers** (Streamable HTTP) for tool integrations.
2. **Declarative HTTP tools**: admin-configured URL, method, auth, and a parameter schema. The LLM fills parameters only; it never chooses URLs.
3. **Packs**: versioned, hash-pinned *data* (inbox adapter mappings, fast-path grammar, MCP and HTTP-tool presets, prompt snippets), installable from the portal. A pack shows the permissions it requests, and cannot disable the confirm policy or untrusted-text handling.
4. **Sidecars**: separate processes off-device (agent bridge, Wi-Fi provisioner, Home Assistant integration, heavy media backends) talking to the documented core API.

The portal installs and manages data and connections only; it never downloads and executes native code. A sandboxed runtime (e.g. WASM) is explicitly deferred and would need a new ADR.

## Consequences

**Positive:** the core stays small and safe; integrations are easy to author and review (they are data or separate programs); the portal can manage everything; the extension surface is the same one third parties already use (MCP, HTTP).
**Negative:** some integrations need a sidecar (extra process off the device); packs and HTTP tools are less expressive than code; we own a pack format and its versioning.
**Follow-ups:** pack format spec (REQ-EXT-002); trust model (REQ-EXT-003); curated index (optional, later); extensibility architecture doc ([extensibility.md](../architecture/extensibility.md)).
