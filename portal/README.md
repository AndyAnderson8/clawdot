# ClawDot portal

The web page served by the ClawDot core on the device. It is a small [Preact](https://preactjs.com) app written in strict TypeScript and built by esbuild into **one self-contained file**, `crates/core/src/ui.html`, which is committed. Node is a development and CI tool only; the device never runs it. Why this stack: [ADR-0016](../docs/adr/0016-portal-typescript-stack.md).

## Run the checks

Needs Node 24 or newer. Install once (two installs: the lint tools need a different TypeScript than the compiler, see `lint/package.json`):

```sh
cd portal
npm ci --ignore-scripts
npm ci --ignore-scripts --prefix lint
```

| Command                | What it does                                                                      |
| ---------------------- | --------------------------------------------------------------------------------- |
| `npm run check`        | Everything below except the browser test: lint, format, types, tests, built page  |
| `npm run lint`         | ESLint, type-aware, zero warnings allowed (`lint:fix` applies the safe fixes)     |
| `npm run format:check` | Prettier says the files are formatted (`npm run format` rewrites them)            |
| `npm run typecheck`    | `tsc` on the sources and on the tests                                             |
| `npm test`             | Unit tests (`node:test`, no browser)                                              |
| `npm run check-build`  | Fails if `crates/core/src/ui.html` differs from what the sources build to         |
| `npm run test:browser` | Headless Chrome against a fake device (needs Chrome, Chromium or Edge)            |
| `npm run licenses`     | Every installed package has a permissive licence (named exceptions in the script) |
| `npm run audit`        | `npm audit` for the shipped dependencies                                          |

`scripts/check.sh` runs the same checks when anything under `portal/` or `ui.html` changed, and CI runs them on every push.

## Build

```sh
npm run build   # writes ../crates/core/src/ui.html
```

Commit the regenerated `ui.html` together with the source change; `check-build` and CI fail when they disagree. The build inlines the script and style, so the page makes no other requests (the core sends a strict Content-Security-Policy).

## Structure

```text
src/
  main.tsx        entry: mounts <App/>, starts the store, refreshes every 10 seconds
  App.tsx         the shell: sign-in and setup screens, the top and bottom bars, the four destinations (TABS)
  tabs/Dashboard.tsx, Reminders.tsx, Activity.tsx   the first three destinations
  tabs/SettingsPage.tsx   Settings: the section list and each section (SECTIONS)
  tabs/Settings.tsx       one group of settings with its own Save (Group)
  tabs/Webhooks, Tools, Connections, Skills, Notes, About   the parts that sections are made of
  dashboard.ts    what the Dashboard says, as pure functions (state words, attention items, recent list)
  snippets.ts     copy-and-paste setup text for inputs; the adapter list
  theme.ts        the light, dark or match-system choice (kept by this browser)
  parts.tsx       small pieces several screens share (Field, CopyButton, badges, go())
  store.ts        what the page knows (one small store) and the actions that change it
  api.ts          the only code that talks to the device (fetch to this origin, bearer token)
  forms.ts        pure functions that turn form values into API request bodies
  time.ts         date and time formatting in the device's time zone
  types.ts        the shapes of the device's JSON API
  tokens.css      design tokens (Astryx neutral names and values, trimmed; see ADR-0017)
  styles.css, index.html   components and the page shell (index.html holds the build markers)
test/unit/        node:test unit tests for api, forms, snippets, dashboard, time and the safety rules
test/browser/     smoke test: a Chrome driver (chrome.ts), a fake device (mock-api.ts), the steps (smoke.ts)
lint/             ESLint config and its own package.json (a separate install, see above)
build.mjs         the esbuild script
```

## Conventions

- **Components** are plain functions that return `JSX.Element` (the return type is required on exported functions). Props are typed inline. Use `class`, `for` and `onClick` as Preact does. Keep a component to one screen's worth of markup; move anything reused to `parts.tsx`.
- **State** lives in `store.ts`. Read it with `useApp()`, change it with `set()` or one of the actions (`load`, `loadExt`, `change`, ...). Form fields keep their own state with `useState` until saved. `usePolled(fn)` runs a tab's loader on mount and on every refresh tick.
- **API client**: call `api<T>(method, path, body)` from `api.ts` and nowhere else; never use `fetch` directly. Show failures with `fail(error)`, success with `say("text")`. Paths start with `/api/`. Add the response type to `types.ts`.
- **Request bodies** are built by pure functions in `forms.ts`, so they can be unit tested without a browser.
- **Text from agents, tools and the network is data.** Render it as JSX text only. `innerHTML`, `dangerouslySetInnerHTML`, `eval` and friends are lint errors (`lint/safety.mjs`), and `test/unit/rules.test.ts` proves the rules still catch them.
- **Types**: no `any`, no non-null `!`, type-only imports use `import type`, `switch` over a union must cover every case. Prefer `unknown` plus a check over a cast.
- **Plain words** in the interface (see REQ-CFG-026): no jargon like "inbox" or "scheduled".
- Style is whatever Prettier outputs (120 columns, double quotes, trailing commas). Do not hand-format; run `npm run format`.

## Add a screen or a settings section

1. A destination (rare; keep to four): create `src/tabs/Name.tsx` exporting `function Name(): JSX.Element` and add `{ id, label, view }` to `TABS` in `App.tsx`. A settings section: add a component to `tabs/SettingsPage.tsx` and an entry to `SECTIONS`; compose it from `Group` forms and the existing parts.
2. Add the response type to `types.ts` and, if it needs a body, a builder to `forms.ts` with a test in `test/unit/forms.test.ts`.
3. Use native controls (`<select>`, `<input type=...>`) and the tokens in `tokens.css`; never a custom popup.
4. Add a step to `test/browser/smoke.ts` and a handler for its endpoints to `test/browser/mock-api.ts`.
5. Add the device side in `crates/core/src/ui.rs` and the requirement to the spec (`docs/specs/`), then `npm run check`, `npm run build`, commit the regenerated `ui.html`.
