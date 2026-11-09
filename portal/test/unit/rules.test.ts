// Source rules that keep the portal safe: agent text is data, nothing leaves this origin, no dynamic code.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((d) => (d.isDirectory() ? files(join(dir, d.name)) : [join(dir, d.name)]));
}
const sources = files(join(root, "src")).filter((f) => /\.(ts|tsx)$/.test(f)).map((f) => ({ f: f.slice(root.length + 1), text: readFileSync(f, "utf8") }));

function none(re: RegExp, why: string, allow: string[] = []) {
  const bad = sources.filter((s) => !allow.includes(s.f.replaceAll("\\", "/")) && re.test(s.text)).map((s) => s.f);
  assert.deepEqual(bad, [], why);
}

test("there is source to check", () => assert.ok(sources.length >= 15));

test("untrusted text is never injected as HTML", () => {
  none(/dangerouslySetInnerHTML|innerHTML|outerHTML|insertAdjacentHTML|document\.write|createContextualFragment|DOMParser/, "no HTML injection APIs");
});

test("no dynamic code", () => {
  none(/\beval\s*\(|new\s+Function\s*\(|setTimeout\s*\(\s*["'`]|setInterval\s*\(\s*["'`]|import\s*\(/, "no eval, Function, string timers, or dynamic import");
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
  none(/\binbox\b|\bscheduled\b|\bradio\b|\bmusic\b|\bstation/i, "plain names only");
});
