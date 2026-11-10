// Source rules that keep the portal safe: agent text is data, nothing leaves this origin, no dynamic code.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((d) =>
    d.isDirectory() ? files(join(dir, d.name)) : [join(dir, d.name)],
  );
}
const sources = files(join(root, "src"))
  .filter((f) => /\.(ts|tsx)$/.test(f))
  .map((f) => ({ f: f.slice(root.length + 1), text: readFileSync(f, "utf8") }));

function none(re: RegExp, why: string, allow: string[] = []) {
  const bad = sources.filter((s) => !allow.includes(s.f.replaceAll("\\", "/")) && re.test(s.text)).map((s) => s.f);
  assert.deepEqual(bad, [], why);
}

test("there is source to check", () => assert.ok(sources.length >= 15));

// The HTML-injection ban is an ESLint rule set (lint/safety.mjs), enforced on the real sources by `npm run lint`.
// This test keeps that safety net honest: it lints known-bad snippets with the same rules and expects every one to be
// reported, and a clean snippet to pass. A rule that stops matching fails here instead of silently allowing a hole.
interface LintResult {
  messages: { ruleId: string | null }[];
}
interface EslintApi {
  ESLint: new (options: object) => { lintText(code: string, options: { filePath: string }): Promise<LintResult[]> };
}
async function lintSnippet(code: string): Promise<(string | null)[]> {
  const load = async (path: string): Promise<unknown> => import(pathToFileURL(join(root, path)).href);
  const eslint = (await load("lint/node_modules/eslint/lib/api.js")) as EslintApi & { default?: EslintApi };
  const { safetyRules } = (await load("lint/safety.mjs")) as { safetyRules: object };
  const { ESLint } = eslint.default ?? eslint;
  const linter = new ESLint({
    cwd: root,
    overrideConfigFile: true,
    overrideConfig: [
      {
        files: ["**/*.jsx"],
        languageOptions: {
          globals: { setTimeout: "readonly", document: "readonly" },
          ecmaVersion: "latest",
          sourceType: "module",
          parserOptions: { ecmaFeatures: { jsx: true } },
        },
        rules: safetyRules,
      },
    ],
  });
  const [result] = await linter.lintText(code, { filePath: join(root, "probe.jsx") });
  return (result?.messages ?? []).map((m) => m.ruleId);
}

test("untrusted text is never injected as HTML or run as code (lint rules catch each way in)", async () => {
  const bad: Record<string, string> = {
    "dangerouslySetInnerHTML prop": `const a = <div dangerouslySetInnerHTML={{ __html: t }} />;`,
    "dangerouslySetInnerHTML in props object": `h("div", { dangerouslySetInnerHTML: { __html: t } });`,
    "innerHTML assignment": `el.innerHTML = t;`,
    "outerHTML assignment": `el.outerHTML = t;`,
    insertAdjacentHTML: `el.insertAdjacentHTML("beforeend", t);`,
    "document.write": `document.write(t);`,
    DOMParser: `new DOMParser().parseFromString(t, "text/html");`,
    createContextualFragment: `range.createContextualFragment(t);`,
    eval: `eval(t);`,
    "new Function": `new Function(t);`,
    "string timer": `setTimeout("go()", 10);`,
    "javascript: URL": `const u = "javascript:alert(1)";`,
    "dynamic import": `import(t);`,
  };
  for (const [what, code] of Object.entries(bad)) {
    assert.notDeepEqual(await lintSnippet(code), [], `${what} must be reported`);
  }
  assert.deepEqual(
    await lintSnippet(`const a = <p>{t}</p>; el.textContent = t;`),
    [],
    "plain text rendering is allowed",
  );
});

test("no dynamic code", () => {
  none(
    /\beval\s*\(|new\s+Function\s*\(|setTimeout\s*\(\s*["'`]|setInterval\s*\(\s*["'`]|import\s*\(/,
    "no eval, Function, string timers, or dynamic import",
  );
});

test("only api.ts talks to the network, and only to this origin", () => {
  none(/\bfetch\s*\(|XMLHttpRequest|WebSocket|EventSource|sendBeacon/, "network calls live in api.ts", ["src/api.ts"]);
  const api = sources.find((s) => s.f.replaceAll("\\", "/") === "src/api.ts")!.text;
  assert.ok(!/https?:\/\//.test(api), "api.ts has no absolute URL");
  for (const s of sources) {
    for (const m of s.text.matchAll(/\bapi(?:<[^>]*>)?\(\s*"[A-Z]+",\s*(["'`])([^"'`]*)\1/g)) {
      assert.ok(m[2]!.startsWith("/api/"), `${s.f}: ${m[2]} is not a /api/ path`);
    }
  }
});

test("no external scripts, fonts, or images are referenced", () => {
  none(/<script[^>]+src=|<link[^>]+href=|@import\s+url|src=\{?["']https?:/, "no external assets");
});

test("the words REQ-CFG-026 forbids stay out of the sources", () => {
  none(/\binbox\b|\bscheduled\b|(?<!type=")\bradio\b|\bmusic\b|\bstation/i, "plain names only");
});
