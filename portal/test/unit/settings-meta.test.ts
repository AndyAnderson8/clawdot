// The page draws each setting's control from what the device says the setting is (kind, limits, choices, live status),
// and keeps only its own wording. These tests tie the page's lists of settings to a sample response, so a key the page
// shows that the device does not describe fails here. The sample is test/fixtures/settings-meta.json, which
// crates/core/src/settings/tests.rs keeps equal to the table the device validates with (REQ-CFG-039).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { hintText, LABEL, settingsPatch, type SettingSpec } from "../../src/forms.ts";
import { META, sampleSettings } from "./sample.ts";

const src = (file: string) => readFileSync(new URL("../../src/" + file, import.meta.url), "utf8");

/** Every setting key a page file names: `key: "name"` in its lists, and `sv("name")` reads. */
function keysIn(file: string): string[] {
  const text = src(file);
  const keys = new Set<string>();
  for (const m of text.matchAll(/\bkey: "([a-z_]+)"/g)) keys.add(m[1]!);
  for (const m of text.matchAll(/\bsv\("([a-z_]+)"\)/g)) keys.add(m[1]!);
  for (const m of text.matchAll(/\bfields\["([a-z_]+)"\]/g)) keys.add(m[1]!);
  return [...keys];
}

const PAGES = [
  "store.ts",
  "tabs/SettingsPage.tsx",
  "tabs/Features.tsx",
  "tabs/Dashboard.tsx",
  "tabs/Webhooks.tsx",
  "tabs/Voice.tsx",
  "tabs/Lights.tsx",
  "tabs/Media.tsx",
];

test("every setting key a page lists is described by the device", () => {
  let seen = 0;
  for (const file of PAGES) {
    for (const key of keysIn(file)) {
      seen++;
      const m = META[key];
      assert.ok(m, `${file} lists "${key}", which the device does not describe`);
      assert.ok(m.kind && typeof m.live === "boolean" && m.group, `${key} lacks kind, live or group`);
    }
  }
  assert.ok(seen > 25, `only ${seen} keys were read from the pages`);
});

test("the pages say how to label every setting they show in a form", () => {
  for (const key of keysIn("tabs/SettingsPage.tsx")) {
    if (META[key]) assert.ok(LABEL[key] !== undefined, `no label for ${key}`);
  }
});

test("limits are described, and only on the kinds that have them", () => {
  for (const [key, m] of Object.entries(META)) {
    if (m.kind === "whole" || m.kind === "money") {
      assert.ok(typeof m.min === "number" && typeof m.max === "number" && m.min <= m.max, `${key} range`);
    } else assert.ok(m.min === undefined && m.max === undefined, `${key} has a range but is ${m.kind}`);
    if (m.kind === "text" || m.kind === "long_text") assert.ok((m.max_length ?? 0) > 0, `${key} max_length`);
    if (m.kind === "choice") assert.ok((m.choices ?? []).length > 1, `${key} choices`);
  }
});

test("the page takes a whole number's range from the device, not from its own hint", () => {
  const s = sampleSettings({ agent_max_rounds: [8, "default"], device_name: ["Dot", "default"] });
  const rounds: SettingSpec = { key: "agent_max_rounds", hint: "How many times it may go back." };
  assert.equal(hintText(rounds, s.fields["agent_max_rounds"]), "How many times it may go back. 1 to 20.");
  assert.equal(
    hintText({ key: "device_name", hint: "Shown at the top." }, s.fields["device_name"]),
    "Shown at the top.",
  );
  assert.equal(hintText({ key: "device_name" }, s.fields["device_name"]), "");
  assert.equal(hintText(rounds, undefined), "How many times it may go back.");
});

test("a value is sent as the kind the device reports: numbers, switches and text", () => {
  const s = sampleSettings({
    agent_max_rounds: [8, "default"],
    daily_spend_usd: [null, "default"],
    dry_run: [false, "default"],
    quiet_start: [null, "default"],
    speech_provider: ["openai", "default"],
  });
  const specs: SettingSpec[] = Object.keys(s.fields).map((key) => ({ key }));
  const cur = Object.fromEntries(specs.map((sp) => [sp.key, ""]));
  assert.deepEqual(
    settingsPatch(s.fields, specs, cur, {
      agent_max_rounds: "5",
      daily_spend_usd: "2.50",
      dry_run: "true",
      quiet_start: "22:00",
      speech_provider: "deepgram",
    }),
    { agent_max_rounds: 5, daily_spend_usd: 2.5, dry_run: true, quiet_start: "22:00", speech_provider: "deepgram" },
  );
});
