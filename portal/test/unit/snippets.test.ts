import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { ADAPTERS, adapterName, snippets } from "../../src/snippets.ts";

test("every input gets a generic set of snippets that name no product, with the placeholder token until a real one is shown", () => {
  const plain = snippets("http://dot:3335", "ci", "generic", undefined);
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
  const real = snippets("http://dot:3335", "ci", "generic", "abc123");
  assert.ok(real.every(([t, s]) => t.startsWith("GitHub") || (s.includes("abc123") && !s.includes("YOUR_TOKEN"))));
});

test("the coding agent adapter adds its own hooks and bridge line after the generic ones", () => {
  const list = snippets("http://dot:3335", "agent", "claude-code", "abc123");
  assert.equal(list.length, 6);
  const hooks = list[4]!;
  assert.equal(hooks[0], "Coding agent hooks: add to ~/.claude/settings.json");
  assert.ok(hooks[1].includes('"url": "http://dot:3335/hook/agent"') && hooks[1].includes("Bearer abc123"));
  JSON.parse(hooks[1]); // valid JSON
  assert.ok(list[5]![1].includes("CLAWDOT_TOKEN=abc123") && list[5]![1].includes("--endpoint agent"));
});

test("the adapter list is neutral: plain JSON first, and coding agent hooks are one item", () => {
  assert.equal(ADAPTERS[0]![0], "generic");
  assert.equal(ADAPTERS.length, 2);
  assert.equal(adapterName("generic"), "Plain JSON");
  assert.equal(adapterName("claude-code"), "Coding agent hooks");
});

// The CLI (`clawdot --create-endpoint`) prints the adapter's `setup` lines; the page builds the same hook JSON and
// bridge line by hand. This keeps the two from drifting apart. The wording around them may differ.
test("the coding agent snippets match the adapter file's setup lines", () => {
  const file = new URL("../../../crates/core/adapters/claude-code.json", import.meta.url);
  const setup = (JSON.parse(readFileSync(file, "utf8")) as { setup: string[] }).setup;
  const fill = (s: string) =>
    s
      .replaceAll("{public}", "http://dot:3335")
      .replaceAll("{endpoint}", "agent")
      .replaceAll("{token}", "abc123")
      .replaceAll("<token>", "abc123");

  const first = setup.indexOf("{");
  const last = setup.indexOf("}");
  assert.ok(first >= 0 && last > first, "the adapter's setup has a JSON block");
  const adapterHooks: unknown = JSON.parse(fill(setup.slice(first, last + 1).join("\n")));
  const adapterBridge = fill(setup.find((l) => l.includes("clawdot-bridge")) ?? "").trim();

  const list = snippets("http://dot:3335", "agent", "claude-code", "abc123");
  assert.deepEqual(JSON.parse(list[4]![1]), adapterHooks);
  assert.equal(list[5]![1], adapterBridge);
});
