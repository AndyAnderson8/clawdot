import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { ADAPTERS, adapterName, adapterSnippets, fillSetup, snippets } from "../../src/snippets.ts";

// What the device sends for an endpoint's `setup`: the adapter file's lines, placeholders unfilled.
const adapterFile = (name: string) =>
  (
    JSON.parse(readFileSync(new URL(`../../../crates/core/adapters/${name}.json`, import.meta.url), "utf8")) as {
      setup: string[];
    }
  ).setup;
const CLAUDE = adapterFile("claude-code");
const GENERIC = adapterFile("generic");

test("every input gets a generic set of snippets that name no product, with the placeholder token until a real one is shown", () => {
  const plain = snippets("http://dot:3335", "ci", "generic", undefined, GENERIC);
  assert.deepEqual(
    plain.map(([t]) => t),
    [
      "curl, from a terminal",
      "PowerShell",
      "GitHub Actions: add as the last step of a job",
      "Home Assistant: add to configuration.yaml",
    ],
  );
  for (const [, text] of plain) assert.ok(text.includes("http://dot:3335/hook/ci"), text);
  assert.ok(plain[0]![1].includes("Bearer YOUR_TOKEN"));
  assert.ok(plain[1]![1].includes("Invoke-RestMethod") && plain[1]![1].includes("Bearer YOUR_TOKEN"));
  assert.ok(plain[2]![1].includes("${{ secrets.CLAWDOT_TOKEN }}"));
  assert.ok(plain[3]![1].includes("rest_command:") && plain[3]![1].includes("to_json"));
  assert.ok(!/claude/i.test(plain.map(([, s]) => s).join("\n")), "generic snippets must not mention a coding agent");
  const real = snippets("http://dot:3335", "ci", "generic", "abc123", GENERIC);
  assert.ok(real.every(([t, s]) => t.startsWith("GitHub") || (s.includes("abc123") && !s.includes("YOUR_TOKEN"))));
});

test("the coding agent adapter adds its own hooks and bridge line after the generic ones", () => {
  const list = snippets("http://dot:3335", "agent", "claude-code", "abc123", CLAUDE);
  assert.equal(list.length, 6);
  const hooks = list[4]!;
  assert.match(hooks[0], /^Claude Code hooks: add to ~\/\.claude\/settings\.json/);
  assert.ok(hooks[1].includes('"url": "http://dot:3335/hook/agent"') && hooks[1].includes("Bearer abc123"));
  JSON.parse(hooks[1]); // valid JSON
  assert.ok(list[5]![1].includes("CLAWDOT_TOKEN=abc123") && list[5]![1].includes("--endpoint agent"));
  const noToken = snippets("http://dot:3335", "agent", "claude-code", undefined, CLAUDE);
  assert.ok(noToken[4]![1].includes("Bearer YOUR_TOKEN") && noToken[5]![1].includes("CLAWDOT_TOKEN=YOUR_TOKEN"));
});

test("an adapter the device does not describe adds nothing", () => {
  assert.equal(snippets("http://dot:3335", "x", "other", "abc123", []).length, 4);
});

test("the adapter list is neutral: plain JSON first, and coding agent hooks are one item", () => {
  assert.equal(ADAPTERS[0]![0], "generic");
  assert.equal(ADAPTERS.length, 2);
  assert.equal(adapterName("generic"), "Plain JSON");
  assert.equal(adapterName("claude-code"), "Coding agent hooks");
});

test("placeholders are filled in one pass and nothing else in braces is touched", () => {
  const values = { public: "http://dot:3335", endpoint: "a{token}b", token: "T" };
  assert.equal(fillSetup("{public}/hook/{endpoint} {x} {{token}}", values), "http://dot:3335/hook/a{token}b {x} {T}");
});

// `clawdot --create-endpoint` prints the adapter's `setup` lines with the placeholders filled in; the page prints the
// same lines from the device. The text exists once, in the adapter file; this keeps the page from changing it.
test("the page prints exactly the adapter's setup lines, filled in", () => {
  const values = { public: "http://dot:3335", endpoint: "agent", token: "abc123" };
  const printed = fillSetup(CLAUDE.join("\n"), values);
  const list = snippets("http://dot:3335", "agent", "claude-code", "abc123", CLAUDE);
  const fromPage = list
    .slice(4)
    .map(([title, text]) => `${title}:\n${text}`)
    .join("\n\n");
  // The bridge line is indented in the printed text and flush in the page; nothing else differs.
  assert.equal(
    fromPage,
    printed
      .trim()
      .split("\n")
      .map((l) => l.replace(/^ {2}(?=CLAWDOT_TOKEN)/, ""))
      .join("\n"),
  );
  // The part titles are the heading lines of the adapter file.
  assert.deepEqual(
    adapterSnippets(CLAUDE, values).map(([t]) => t + ":"),
    CLAUDE.filter((l) => l.endsWith(":")),
  );
});

test("an address that ends with a slash does not give a double slash", () => {
  const out = snippets("http://dot.local:3335/", "ci", "generic", "tok", []);
  assert.ok(out.every(([, text]) => !text.includes("3335//")));
  assert.ok(out[0]![1].includes("http://dot.local:3335/hook/ci"));
});
