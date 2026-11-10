# ADR-0016: Portal source language and framework: TypeScript with Preact, built by esbuild, output committed

- **Status:** Proposed
- **Date:** 2026-11-09
- **Deciders:** Maintainer
- **Addendum (2026-11-09):** styling and the component set are decided in [ADR-0017](0017-portal-design-system-astryx.md) (the Astryx token contract with our own native-control components). The framework decision here stands.

## Context

The web portal is one hand-written file, `crates/core/src/ui.html`, plus `ui-theme.css`: about 70 KB of HTML, CSS and untyped JavaScript with eight tabs (Home, Reminders, Agents, Tools, Connections, Skills, Webhooks, Settings) and the setup and sign-in screens. It is embedded in the binary, gzipped at build time by `build.rs` (REQ-CFG-043), and served under a strict Content-Security-Policy. The earlier comparison in [ui-stack.md](../design/ui-stack.md) recommended TypeScript with no framework and revisited Preact "when the UI passes roughly eight screens". That trigger has been reached: the page now has eight tabs and every redraw is hand-written (`replaceChildren`, with state kept in closures and in the DOM). The maintainer prefers TypeScript and wants a lightweight framework so the portal is pleasant to change.

This ADR replaces opinion with measurement. The same tab (Reminders: list, add, edit, cancel, against the existing `/api/schedule` JSON API, with the existing styles and behaviour) was built five times and measured. The shared part (typed API client, time formatting, request building: 72 lines) is identical in all of them; only the UI layer differs.

Constraints that do not move: the device is an Echo Dot Gen 2 class machine with a 48 MB budget ([spec 0002](../specs/0002-resource-budget.md), [ADR-0003](0003-reference-hardware-budget.md)), but the page runs in the user's phone, so what we pay on the device is bytes in the binary and bytes on the wire; the repository must keep building with Rust alone; agent text is untrusted and must render as text only; no external requests and no `eval`.

## Decision drivers

- Wire size (gzip) and binary size; the page is sent from a small device over Wi-Fi.
- Type safety for the API payloads, and a UI layer that is declarative enough that state and DOM cannot drift apart.
- Supply chain: few packages, permissive licences, reproducible `npm ci`, nothing that runs install scripts.
- A one-person project: skills that transfer, one obvious way to do things, a toolchain that rarely breaks.
- CSP: no `eval`, no inline event handler attributes, no external requests.
- Rust-first repository: `cargo build` and `cargo install` must not need Node.

## Options considered and measured

All five were built with esbuild 0.28.2 (minified, IIFE, ES2022) and inlined into the real page shell and styles. Each passed the same ten browser checks in headless Chrome (list renders, agent-supplied label `<b>work</b>` stays text, repeat text, edit loads the form, PUT body, leaving edit mode, DELETE, alarm POST with days, validation message, every input labelled) against a mock API.

| Option | Script raw | Script gzip | Script brotli | Full page gzip | Overhead vs vanilla (gzip) | UI source lines | Build time | JS heap after render (indicative) |
|---|---|---|---|---|---|---|---|---|
| a. Vanilla TypeScript + 25-line typed `h()` helper | 6,649 B | **2,977 B** | 2,633 B | 6,692 B | n/a | 80 | 58 ms | 0.9 MB |
| b. **Preact 11 + JSX** (esbuild's own JSX transform) | 19,864 B | **8,280 B** | 7,492 B | 12,070 B | +5.3 KB | 78 | 26 ms | 1.4 MB |
| c. Lit 3 (web component, light DOM) | 21,771 B | **8,416 B** | 7,586 B | 12,173 B | +5.4 KB | 71 | 22 ms | 2.4 MB |
| d. Svelte 5 (compiler called from a 10-line esbuild plugin) | 66,381 B | **25,162 B** | 22,700 B | 28,972 B | +22.2 KB | 71 | 89 ms | 2.6 MB |
| e. Solid 1.9 (Babel preset, run from an esbuild plugin) | 22,030 B | **8,897 B** | 8,083 B | 12,690 B | +5.9 KB | 77 | 388 ms | 3.4 MB |

Notes: "full page" is the inlined result with theme and styles (the real shell is larger). Build time is one in-process esbuild call, warm. Heap is one `performance.memory` sample per run and is noisy; it is secondary here because the page does not run on the device. For scale, today's page is 70,557 B raw, 20,446 B gzip, 17,645 B brotli (script alone: 55,750 B raw, 16,005 B gzip).

### Supply chain, licences, tooling

| Option | npm packages installed (excluding other options) | Licences | Needs beyond esbuild | Install scripts |
|---|---|---|---|---|
| a. Vanilla TS | 0 | n/a | none | none |
| b. Preact | 1 (`preact`, no dependencies) | MIT | none: esbuild compiles JSX | none |
| c. Lit | 5 (`lit`, `lit-html`, `lit-element`, `@lit/reactive-element`, `@lit-labs/ssr-dom-shim`) | BSD-3-Clause | needs `useDefineForClassFields: false` and no decorators, or a decorator transform | none |
| d. Svelte | 19 (acorn, magic-string, esrap, aria-query, devalue, ...) | MIT and Apache-2.0 | a custom esbuild plugin around `svelte/compiler`; `.svelte` files are a second language with their own type-check tool | none |
| e. Solid | about 52, almost all Babel | MIT | Babel 7 plus `babel-preset-solid`; the preset's peer range did not accept the current Babel 8, so `npm install` needed `--legacy-peer-deps`; types are stripped by esbuild first, then JSX compiled by Babel | none |

The build tools common to all: `esbuild` (MIT, 2 packages: the launcher and one platform binary, no dependencies) and `typescript` 7 (Apache-2.0, 2 packages; the native compiler type-checked all prototypes in 2.1 s). Nothing in any tree is copyleft. `cargo deny` does not see npm, so the npm packages get their own allow-list check (below).

### Qualitative comparison

| Question | a. Vanilla | b. Preact | c. Lit | d. Svelte | e. Solid |
|---|---|---|---|---|---|
| Accessibility ergonomics | All ARIA, focus and label wiring by hand (the existing page does it well, but nothing helps) | Declarative `aria-*`, `for`, roles in JSX; focus needs a ref; optional `eslint-plugin-jsx-a11y` | Shadow DOM would break `label for`/`id` pairs and the global theme; the light-DOM workaround removes the main reason to use components | Best: the compiler warns about missing labels and bad roles | As Preact, no compile-time help |
| Test story | Pure logic with `node:test`; DOM only with a browser | Same, plus any DOM harness; components are plain functions | Needs a real or emulated browser for custom elements | Needs a Svelte-aware runner for component tests | Needs Babel in the test path |
| Maintainability for one person | The prototype's edit mode is 12 lines of "copy fields into inputs"; every new field means touching state, layout and reset in sync | State is the single source; the form is a function of state; the most widely known model | Two ways to write it (decorators or not), a TypeScript trap that silently stopped updates until `useDefineForClassFields` was set | A second language and file type; smaller runtime surface but a big runtime (see size) | Reactivity rules (no destructuring props, signal calls) that bite newcomers; Babel in the chain |
| Template type safety | Full (it is TypeScript) | Full: JSX is type-checked | None inside `html` templates | Partial (markup is not checked by `tsc`) | Full |
| CSP | No `eval`, no `new Function`, no external URLs in the bundle | Same | Same | Same; the bundle holds about two dozen `https://svelte.dev/e/...` strings that are never fetched | Same |

CSP result for all five: no `eval`, no `new Function`, no inline event-handler attributes (handlers are set as properties or by the framework through `addEventListener`), no network calls other than `fetch` to the page's own origin, so `connect-src 'self'` and the existing policy hold unchanged.

## Decision

1. **Language:** TypeScript, `strict`, with `erasableSyntaxOnly` so the same sources run under `node --test` with Node's built-in type stripping (no transpile step for tests).
2. **Framework: Preact 11 with JSX, compiled by esbuild.** It is the smallest runtime that gives a declarative UI (+5.3 KB gzip over hand-written TypeScript for the whole portal, paid once, not per tab), it has no dependencies, it needs no plugin, JSX is checked by `tsc`, and it is the model most people (and most future contributors) already know. Lit and Solid cost about the same bytes but add friction (shadow DOM versus the theme; Babel and a peer-range conflict). Svelte is four times larger on the wire and brings a compiler and 19 packages. Hand-written TypeScript is smallest but keeps the problem that prompted this work: every redraw is bookkeeping.
3. **Rendering rule stays:** components pass untrusted text only as JSX children or attributes (Preact escapes them). `dangerouslySetInnerHTML` is forbidden by a lint check in the portal test suite (a search over `portal/src`), the same rule the hand-written page kept by convention.
4. **Hooks only from `preact/hooks`;** no `preact/compat`, no router, no state library, no CSS-in-JS. The theme and styles stay plain CSS ([brand.md](../design/brand.md), [portal.md](../design/portal.md)).

### Build pipeline (Rust-first)

- **Node is a development and CI tool only.** `cargo build`, `cargo install` and the release workflow never run Node.
- **Built output is committed.** `portal/` holds the sources; `npm run build` writes the finished page to `crates/core/src/ui.html` (generated, with a header comment saying so). `build.rs` keeps doing what it does today for REQ-CFG-043 (read the page, write a gzip copy) and no longer splices a theme. This keeps the repository reproducible from Rust alone and keeps the pre-compressed copy the server already serves.
- **A CI job rebuilds and fails on any difference** (`git diff --exit-code crates/core/src/ui.html`), so the committed page can never drift from its sources.
- **Reproducibility and hygiene:** exact versions in `package.json` (no `^`), `package-lock.json` committed, `.nvmrc`/`engines` pinning the Node major (24, the current LTS line), CI uses `npm ci --ignore-scripts` (esbuild and TypeScript ship their binaries as optional platform packages and do not need install scripts). A small script checks every installed package's licence against an allow-list (MIT, Apache-2.0, BSD-2/3-Clause, ISC, 0BSD) and `npm audit --omit=dev --audit-level=high` runs in CI. Dependabot can later watch `portal/`.
- **Keeping `scripts/check.sh` fast:** the portal steps (type-check, unit tests, build-is-current) run only when files under `portal/` or the generated page changed relative to `dev` (always in CI); the browser smoke test runs in the full gate and CI only. Measured on the prototype: type-check about 2 s, unit tests 0.14 s, build 0.1 s.
- **Serving:** unchanged. One inline page (inline script and style, matching `script-src 'unsafe-inline'`), with the build-time gzip copy chosen by `Accept-Encoding`. Brotli would save about 14 percent more on the wire (17.6 KB versus 20.4 KB today), but it needs a committed binary or a new Rust dependency, so it is deferred. Tightening to `script-src 'self'` (serving the script as its own file) is a worthwhile follow-up and is independent of this choice.

### Test story

- **Unit tests:** `node --test` on the TypeScript sources directly (pure logic: time formatting, request building, snippets, settings diffing). No Vitest, no jsdom, no extra packages.
- **Browser smoke test:** a dependency-free script drives headless Chrome through the DevTools protocol using Node's built-in `WebSocket`, against a mock of the JSON API. It is the harness that produced the numbers above, and it closes the long-standing gap noted in [traceability.md](../quality/traceability.md) ("checked by hand in headless Chrome"). It needs a Chrome binary (present on GitHub's Ubuntu runners) and is skipped locally when none is found.

## Consequences

**Positive:** typed API payloads and typed templates; one idiom for every tab; no more drift between state and DOM; the page remains one inline file under the existing CSP; dependency growth is two build tools and one runtime package (`esbuild`, `typescript`, `preact`), versus an unbounded tree for the alternatives.
**Negative:** a Node toolchain for anyone who edits the portal; roughly +5 KB gzip on the wire and a few KB of binary; a committed generated file (reviewers must read the sources, not the output); one more CI job.
**Follow-ups:** the measured whole-portal size after the port is recorded in the amendment to this ADR with the implementation; consider `script-src 'self'` and a brotli copy later; decide whether to add `eslint-plugin-jsx-a11y` once the component set stabilises.

## How to reproduce

The prototypes lived in a scratch directory and are not committed. Each is about 70 lines of UI code over a shared 72-line `common.ts`; the build is one `esbuild.build({ bundle, minify, format: "iife", target: "es2022" })` call (Preact: `jsx: "automatic", jsxImportSource: "preact"`; Svelte and Solid: an `onLoad` plugin calling `svelte/compiler` or Babel). Sizes are `zlib.gzipSync(level 9)` and `brotliCompressSync(quality 11)` of the minified script, and of the whole inlined page. The real implementation (`portal/`) ships the browser smoke harness, so the method is reproducible there.

## Amendment: measured result of the full port (2026-11-09)

The whole portal (setup, sign-in, banner, and all eight tabs) was ported to `portal/`. Measured on the built page, which `build.rs` still gzips at build time:

| | Before (hand-written) | After (TypeScript, Preact) | Change |
|---|---|---|---|
| Page, raw | 70,557 B | 70,423 B | -134 B |
| Page, gzip (what is sent on the wire) | 20,446 B | 24,241 B | **+3,795 B (+18.6%)** |
| Page, brotli (not served) | 17,645 B | 21,385 B | +3,740 B |
| Script, minified, raw / gzip | 55,750 B (not minified) / 16,005 B | 60,027 B / 20,815 B | Preact and hooks are 13.3 KB of it |
| Style, raw / gzip | about 10 KB / 2.6 KB | 8,708 B / 2,633 B | minified |
| Source | 950 lines in one file | about 1,700 lines in 25 files (incl. CSS and shell) | typed, one file per tab |

The overhead is lower than the prototype's +5.3 KB because minification now shrinks the application code that used to ship as written. Build takes about 0.1 s of esbuild time; type-check about 2 s; 27 unit tests run in 0.25 s; the 15-scenario browser smoke test takes about 20 s. Installed npm packages: 7 in total (`preact`, `esbuild` with its platform binary, `typescript` with its platform binary, `@types/node` with `undici-types`), all MIT or Apache-2.0, and `npm audit` reports none.

Findings worth keeping: the browser test found a real race in the first version (a fast answer from the device could arrive between rendering and the effect that subscribes to the store, leaving a blank page); the store now catches up when it subscribes. The left arrow on the first tab did not wrap in the old page; it does now.
