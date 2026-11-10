# Proposal: the web UI stack

Status: **Superseded in part (2026-11-09)** by [ADR-0016](../adr/0016-portal-typescript-stack.md), which measured five implementations of one tab (vanilla TypeScript, Preact, Lit, Svelte, Solid) and proposes TypeScript with Preact built by esbuild, output committed, no `build.rs` Node dependency. The build-pipeline reasoning below (committed output plus a CI rebuild check, no Node in `cargo build`) still holds; the recommendation of option B (no framework) was changed to option C because the portal reached eight tabs. This note is kept as the original comparison.

Update (2026-11-09): the design system and component approach are in [ADR-0017](../adr/0017-portal-design-system-astryx.md).

## Context

The embedded UI is one hand-written file, `crates/core/src/ui.html`, plus the theme `crates/core/src/ui-theme.css`, both pulled into the binary with `include_str!` (about 50 KB together after the tabbed portal, uncompressed; it was 18 KB). It has no dependencies, makes no external requests, and runs under a strict Content-Security-Policy (inline script and style only, `connect-src 'self'`). The device is a 48 MB-budget Echo Dot Gen 2 class machine (REQ-NFR-101..103, binary at most 8 MB), and the repository builds with Rust only. The UI will grow (history, voice and persona settings, connect-an-agent flow, `/display`), and the maintainer prefers TypeScript. This note compares three ways forward.

## Options

### A. Raw HTML, CSS, and JavaScript (today)

- **Toolchain:** none. Edit the file, `cargo build`.
- **Size and memory:** smallest possible. The browser does the work, not the Dot; the Dot only serves a few KB of static bytes. JS source is shipped as written.
- **Cost of growth:** DOM updates are hand-written (`replaceChildren`, `el()` helpers). A UI of five or six screens is still manageable; beyond that the code gets repetitive and untyped bugs (wrong field names in API payloads) appear only at run time.
- **Safety:** easy to keep the "agent text is rendered as text only" rule, because there is no templating layer.

### B. TypeScript compiled by esbuild, no runtime framework

- **Toolchain:** Node and one dev dependency (`esbuild`, a single static binary, or `typescript` for type-checking only with `tsc --noEmit`). The output is plain JS, bundled and minified into one file.
- **Size:** about the same as A, often smaller after minification (a 12 KB script minifies to roughly 6 to 7 KB). No runtime added.
- **Gain:** types for the API payloads, which can be generated or hand-written once and checked in CI. No framework to learn; same small `el()` helpers, now typed.
- **Cost:** a build step and a Node dependency, discussed below.

### C. TypeScript with Preact (about 4 KB gzipped, roughly 11 KB minified)

- **Toolchain:** as B, plus `preact` (and `htm` or JSX transform).
- **Size:** adds about 4 KB gzipped (about 11 KB raw) of runtime. The binary grows by that amount, not the Dot's RSS in a meaningful way; the framework runs in the user's phone browser.
- **Gain:** components, state, and re-rendering for a UI that is mostly forms, lists and live status. Fewer hand-written DOM updates.
- **Cost:** a second dependency to audit (supply chain), JSX or tagged templates to learn, and a risk that someone uses an unsafe HTML-injection API with agent text. That rule would have to be enforced by lint.

## Build-toolchain cost for a Rust repository

| Question | Notes |
|---|---|
| Node in CI | One extra setup step in the `gate` job (`actions/setup-node` pinned to an LTS) plus `npm ci` from a lock file. Adds roughly 20 to 40 seconds. `cargo deny` does not cover npm, so add `npm audit` (or `osv-scanner`) and a licence check for the few packages involved. |
| Committed or built assets | **Commit the built output** (`ui.html`/`ui.js`, minified) next to the sources, and have CI rebuild and fail if the committed output differs (`git diff --exit-code`). Reason: `cargo build` and `cargo install` must keep working with Rust alone, and the release workflow and WSL development setup stay unchanged. Contributors who do not touch the UI never need Node. |
| `build.rs` | Not recommended. A `build.rs` that calls Node makes every Rust build depend on Node, breaks cross-compiles to armv7, and slows `cargo check`. Keep the bundling as an explicit `scripts/build-ui.sh` that developers run, with the CI diff check as the safety net. |
| Reproducibility | Pin Node, `esbuild`, and (for C) `preact` versions in `package-lock.json`; commit the lock file; builds are deterministic for a given version set. |
| Repo layout | `ui/` with `src/*.ts`, `package.json`, `tsconfig.json`; output copied to `crates/core/src/ui.*`. The existing theme file moves to `ui/src/theme.css` unchanged (it is plain CSS on purpose). |
| Tests | Type-checking (`tsc --noEmit`) replaces a class of bugs. DOM behaviour needs a browser test (see the gap noted in the traceability matrix); a headless-browser smoke test could be added later, but it is a separate decision. |

## Size and memory on the Dot

- The Dot serves the page; it does not run it. Cost on the device is binary size and a few KB copied per page load (`reply` already allocates the page once per request; caching the assembled string would remove even that).
- Budget impact against the 8 MB binary limit: A 0 KB, B about -5 KB to +0, C about +11 KB raw. All are negligible against REQ-NFR-103, but REQ-NFR-114 (ratchet) means the measured numbers should be recorded when a choice is made.
- Compression: the server does not gzip today. Serving a pre-compressed asset would cut the page by about 70 percent on slow Wi-Fi; that is independent of the stack and worth doing with B or C.

## Recommendation

Adopt **B, TypeScript compiled by esbuild with no runtime framework**, with committed build output and a CI rebuild check; no `build.rs`.

Why: it gives the maintainer the TypeScript they prefer and the main benefit that matters here (checked API types) at a cost of zero runtime bytes, one Node dev dependency, and a script. It keeps the CSP and the "text only" rendering rule trivially intact, and leaves the Rust build untouched.

Revisit **C (Preact)** when the UI passes roughly eight screens, or when hand-written DOM updates become the main source of bugs. Moving from B to C later is incremental: the typed API layer and the theme carry over unchanged.

Not recommended now: heavier frameworks (React, Svelte with its compiler, Vue), CSS frameworks, or any CDN or font download (they break offline use and the CSP).

## Decision needed from the maintainer

1. B or C (or stay on A)?
2. Committed build output with a CI rebuild check (recommended), or build in CI only?
3. When to start: with the next UI feature (history, settings), not as a standalone migration.

When decided, record it as an ADR and amend REQ-CFG-001 notes if the shape of the embedded assets changes.
