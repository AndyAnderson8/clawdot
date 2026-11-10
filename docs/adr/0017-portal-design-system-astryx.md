# ADR-0017: Portal design system: the Astryx token contract on Preact, native controls, no Astryx runtime

- **Status:** Proposed
- **Date:** 2026-11-09
- **Deciders:** Maintainer

## Context

The maintainer judged the first portal design poor and asked that the redesign "use Astryx" ([meta/astryx](https://github.com/meta/astryx), MIT, Meta). Astryx is a design system of 150+ React components styled with StyleX, shipped as pre-built CSS (`@astryxdesign/core/astryx.css`) plus ES modules, with themes expressed as CSS custom properties (`@astryxdesign/theme-neutral`, one of seven themes) and a CLI (`@astryxdesign/cli`) for component docs, page templates and theme builds. Version measured: 0.7.11 (the project describes itself as beta).

The portal is served from, and embedded in, a binary for devices of Echo Dot Gen 2 class ([ADR-0003](0003-reference-hardware-budget.md)), and is a Preact app ([ADR-0016](0016-portal-typescript-stack.md)) that is 24 KB gzip today. Astryx targets React 19 and StyleX. The question is how to use it, and whether any way of using it fits. The numbers below were measured, not estimated. The same maintainer feedback also asks for native-looking dropdowns, light by default, and a result that does not look generated.

## What was measured

One screen was built with real Astryx components (Theme with the neutral theme, Button, TextInput, NumberInput, Selector, TimeInput, DateInput, CheckboxInput, Switch, TextArea, Badge, Banner, Heading, Text, VStack, HStack: the form controls of the Reminders and Settings screens, 17 imports) and bundled with esbuild 0.28.2 (minified, IIFE, ES2022), inlined with `astryx.css` and the neutral `theme.css` into one page the way the real build inlines ours, then loaded in headless Chrome served with the device's real Content-Security-Policy. Sizes are `gzip -9` and brotli quality 11 of the whole inlined page.

| | Page raw | Page gzip | Page brotli | JS gzip | CSS gzip | Build time |
|---|---|---|---|---|---|---|
| **a.** Astryx on Preact 11 through `preact/compat` aliasing (needs a shim, see below) | 1,105,264 B | **313,910 B** | 253,560 B | 271,017 B | 42,827 B | 2.6 s |
| **b.** Astryx on React 19 and react-dom 19, tree-shaken by esbuild | 1,327,794 B | **377,441 B** | 306,444 B | 334,545 B | 42,827 B | 3.0 s |
| a, one Button only | 563,841 B | 137,121 B | 113,092 B | 94,231 B | 42,827 B | 1.5 s |
| b, one Button only | 766,950 B | 197,001 B | 163,707 B | 154,079 B | 42,827 B | 1.7 s |
| **c.** Astryx neutral theme tokens only (CSS), with our own Preact components | portal today 70 KB, plus the tokens | **about 28 KB** (24,241 B today plus 3,588 B for the whole neutral `theme.css`; less once trimmed to the tokens we use) | n/a | unchanged (Preact) | 3.6 KB for all 198 tokens | 0.1 s |

Why a and b are this large for a form: `astryx.css` is one pre-built file (269 KB raw, 42.8 KB gzip) for all 150 components, so its CSS cost is paid even for one Button. On the JS side the floor is the theme runtime (78 KB raw), the layering, gesture, motion and keyboard modules every control pulls in, `react-dom` (210 KB raw) or its Preact replacement, and an ICU message-format parser used for translations. A single Button costs 137 KB gzip on Preact. The ten-component screen is already 1.3 to 1.5 times the ceiling proposed below (3.1 to 3.8 times the target) before it grows to the 25 or so distinct components four screens need. (Not measured: the whole portal rewritten in Astryx. It can only be larger than this screen.)

### Findings per option

| Question | a. Preact compat | b. React 19 | c. Tokens only |
|---|---|---|---|
| Works? | Only with a shim. `preact/compat` lacks `use` and `useOptimistic`, which 80 and 16 Astryx modules import; a 14-line shim let every component render and the Selector open. `use(promise)` cannot be shimmed, so async button actions would break. Not a configuration either project documents. | Yes | Yes (no Astryx code runs) |
| Browser smoke test under the real CSP | Rendered, Selector opened, **0 console errors, 0 CSP violations, 0 external requests** | Same: 0, 0, 0 | Same page as today |
| CSP and StyleX | The CSS is pre-built, so StyleX needs no build plugin and no `eval`. The runtime sets some inline `style` attributes, which `style-src 'unsafe-inline'` (the current policy) allows; a stricter `style-src 'self'` would refuse them. | Same | Plain CSS; works under a stricter policy too |
| Native controls | **No.** `Selector` is a custom listbox popup (on touch a bottom sheet); `DateInput` and `TimeInput` are custom pickers ("Select a date"). 0 native `<select>` elements in the page. This is the exact complaint. | Same | Yes: real `<select>`, `<input type=date|time|checkbox|radio>` |
| Accessibility | Components declare roles and labels (combobox, listbox, option were present) and the project documents keyboard behaviour; not audited further here | Same | Our own, tested in the smoke test; native controls carry the platform's behaviour |
| Binary effect (armv7; page is embedded raw and gzipped) | +1.1 MB raw and +0.31 MB gzipped, about 1.4 MB of the 8 MB cap | +1.3 MB and +0.38 MB, about 1.7 MB | about +0.1 MB |
| Build dependencies | 19 packages, 109 MB in `node_modules` (see licences) | same, plus `react-dom` | none new (tokens vendored) |
| Style fit | A "default SaaS kit" look (pill banners, soft radii, shadows) unless a theme is written | Same | Ours; the neutral values give the restrained monochrome base |

### Dependencies and licences (a and b)

All permissive, none copyleft: `@astryxdesign/core` MIT, `@astryxdesign/theme-neutral` MIT, `@stylexjs/stylex` MIT, `intl-messageformat` BSD-3-Clause, `@formatjs/fast-memoize`, `@formatjs/icu-messageformat-parser` and `@formatjs/icu-skeleton-parser` MIT, `lucide-react` ISC, `react` and `react-dom` and `scheduler` MIT, `css-mediaquery` declared as plain "BSD" (ambiguous; would need a named exception in `portal/scripts/licenses.mjs`), `invariant`, `loose-envify`, `js-tokens`, `styleq` MIT. The CLI (`@astryxdesign/cli`, MIT, 13.7 MB, 15 direct dependencies including Babel, PostCSS and jscodeshift) is a development tool and is not needed to ship.

## Soft wire budget for the portal

- **Target: 100 KB gzip for the whole page. Ceiling: 250 KB gzip.** Embedded size (raw plus gzip copies in the binary): at most 600 KB, 7.5% of the 8 MB cap.
- Why: the page is fetched by a phone from a device on home Wi-Fi. A Dot-class radio holds 5 to 20 Mbit/s near the router and about 2 Mbit/s through walls; 250 KB is about 1 s at 2 Mbit/s and 100 KB is 0.4 s, and the page is fetched once per visit and cached by nothing (it is one inline file). The Dot itself does no compression work (the gzip copy is made at build time), so the cost is time on the wire and bytes in a size-capped binary, not CPU or RSS. Today's 24 KB leaves room; the redesign is allowed to grow, but not by an order of magnitude.
- Verdict: **a (314 KB for one screen) and b (377 KB) exceed the ceiling and the target. c (about 28 KB today) meets both.** A brotli copy (about 19% smaller than gzip) would not rescue a or b either: 254 KB and 306 KB.

## Decision

1. **Do not ship Astryx components or the Astryx runtime.** They break the budget by 1.3 to 1.5 times the ceiling for a ten-component screen, need an unsupported shim on Preact, and replace the native dropdowns, date and time inputs the maintainer asked for with custom popups.
2. **Use Astryx as the design system's contract, which is what its own docs say a theme is: a set of CSS custom properties.** The portal's stylesheet adopts the Astryx neutral theme's token names and values (`--color-background-surface`, `--color-text-primary`, `--color-text-secondary`, `--color-border-emphasized`, `--color-accent`, `--color-text-error`, `--radius-*`, `--font-size-*`, `--font-family-*` and the status colours), vendored as a trimmed `portal/src/tokens.css` (only the tokens used, flattened to plain variables with a `[data-theme="dark"]` override block instead of `light-dark()` and `@scope`, so older phones work and the CSS is about 1 KB gzip), with the MIT notice kept in the file header and in `NOTICE`. The ClawDot overrides are made the Astryx way, as overrides of those same properties: the live-state colour (Lagoon `#0b6e63`) and the font stack without the webfont (the page may not load fonts).
3. **Keep Preact and write our own small components** on those tokens, using native form elements, with Astryx's published guidance (component docs, its settings and dashboard page templates, via the CLI run on a developer machine) as the reference for spacing, states and accessibility patterns. [ADR-0016](0016-portal-typescript-stack.md) stands unchanged; this ADR adds the styling and component decision it left open.
4. Because the names match, replacing one of our components with the real Astryx one later is a local change. Reconsider if Astryx ships tree-shakeable, framework-neutral CSS per component, or if the portal moves off the device to a hosted page where the budget no longer applies.

## Consequences

**Positive:** the maintainer's instruction is followed in the way that is measurable (tokens, naming, theme mechanism and guidance), the page stays about 28 KB gzip plus the new screens, native controls everywhere, no new runtime dependency, no new supply chain, light default with a real dark theme.
**Negative:** we maintain a small component set (about 15 small functions) ourselves; the page does not get the 150 components or Astryx's updates; the vendored tokens must be refreshed by hand (a script in the portal README names the file to diff against `@astryxdesign/theme-neutral/dist/theme.css`).
**Risk:** Astryx is at 0.7 and beta, so its token names may change; vendoring means an upstream rename does not break us.

## How to reproduce

Scratch project outside the repository: `npm i react@19 react-dom@19 @astryxdesign/core @astryxdesign/theme-neutral @stylexjs/stylex preact@11.2.0 esbuild@0.28.2 --ignore-scripts`. One file imports the components listed above inside `<Theme theme={neutralTheme}>`; one esbuild call (`bundle`, `minify`, `format: "iife"`, `jsx: "automatic"`, `define process.env.NODE_ENV`), with `alias` of `react`, `react-dom`, `react-dom/client` and `react/jsx-runtime` to `preact/compat` (plus a shim exporting `use` and `useOptimistic`) for option a. The CSS is `astryx.css` and the neutral `theme.css` run through esbuild's CSS minifier. The page is served by a 10-line Node server that sets the Content-Security-Policy from `crates/core/src/ui.rs`, and loaded with the portal's own browser driver (`portal/test/browser/chrome.ts`) while collecting console errors, CSP violations and requests.

## Amendment: measured result of the implementation (2026-11-09)

The redesigned portal (Dashboard, Reminders, Activity, eight Settings sections, sign-in and setup) as built, with tokens and native controls:

| | Before (ten tabs) | After | Change |
|---|---|---|---|
| Page raw | 90,034 B | 104,176 B | +14,142 B |
| Page gzip (on the wire) | 30,248 B | 33,399 B | +3,151 B (+10.4%) |
| Page brotli (not served) | 26,644 B | 29,329 B | +2,685 B |

33.4 KB gzip is a third of the 100 KB target and 13% of the 250 KB ceiling. The new Dashboard, the section layout, the dark theme and the native-control styling cost 3.2 KB gzip together; the Astryx options measured above would have cost 280 to 350 KB on top. Build time is about 0.1 s. The browser test (24 steps) checks that every page has no horizontal scroll at 360 px, that every select is a real element with the platform appearance, and that no custom dropdown role exists.
