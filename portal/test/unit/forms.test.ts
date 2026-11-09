import assert from "node:assert/strict";
import { test } from "node:test";
import {
  connectionDefinition, currentText, labelOf, reminderBody, restartText, settingsPatch, skillBody, snippets, toolDefinition,
  type ReminderForm, type SettingSpec,
} from "../../src/forms.ts";
import type { Settings } from "../../src/types.ts";

const base: ReminderForm = { kind: "timer", label: "  tea ", amount: "5", unit: "60", time: "07:00", days: [], date: "", time2: "" };

test("timer body is seconds, trimmed label", () => {
  assert.deepEqual(reminderBody(base), { kind: "timer", label: "tea", seconds: 300 });
  assert.deepEqual(reminderBody({ ...base, amount: "1.5", unit: "3600" }), { kind: "timer", label: "tea", seconds: 5400 });
});

test("alarm body has hour, minute, and the chosen days", () => {
  assert.deepEqual(reminderBody({ ...base, kind: "alarm", time: "06:30", days: [0, 2] }), { kind: "alarm", label: "tea", hour: 6, minute: 30, days: [0, 2] });
  assert.equal(reminderBody({ ...base, kind: "alarm", time: "" }), "Pick a time for the alarm.");
});

test("reminder body needs a date and a time", () => {
  assert.deepEqual(reminderBody({ ...base, kind: "reminder", date: "2026-12-01", time2: "09:15" }), { kind: "reminder", label: "tea", date: "2026-12-01", time: "09:15" });
  assert.equal(reminderBody({ ...base, kind: "reminder", date: "", time2: "09:15" }), "Pick a date and a time for the reminder.");
  assert.equal(reminderBody({ ...base, kind: "reminder", date: "2026-12-01", time2: "" }), "Pick a date and a time for the reminder.");
});

test("tool definition", () => {
  const f = { name: " set_light ", description: " Turn a light on ", method: "POST", url: " https://h/{room} ", tokenVar: " HA_TOKEN ", confirm: true, params: [{ id: 1, name: "room", type: "string", required: true, description: "" }, { id: 2, name: "on", type: "boolean", required: false, description: " yes or no " }] };
  assert.deepEqual(toolDefinition(f), {
    name: "set_light", description: "Turn a light on", method: "POST", url: "https://h/{room}", confirm: true, auth_env: "HA_TOKEN",
    params: { room: { type: "string", required: true }, on: { type: "boolean", required: false, description: "yes or no" } },
  });
  assert.equal(toolDefinition({ ...f, params: [{ id: 3, name: " ", type: "string", required: true, description: "" }] }), "Give each value a name, or remove it.");
  assert.equal("auth_env" in (toolDefinition({ ...f, tokenVar: " " }) as object), false);
});

test("connection definition", () => {
  assert.deepEqual(connectionDefinition({ name: " home ", url: " https://h/mcp ", tokenVar: "", confirm: false }), { name: "home", url: "https://h/mcp", confirm: false });
  assert.deepEqual(connectionDefinition({ name: "home", url: "u", tokenVar: "T", confirm: true }), { name: "home", url: "u", confirm: true, auth_env: "T" });
});

test("skill body splits tools on commas and spaces", () => {
  assert.deepEqual(skillBody(" Good night ", "Turn off lights", "set_light, set_alarm  x", true), { name: "Good night", instructions: "Turn off lights", tools: ["set_light", "set_alarm", "x"], enabled: true });
  assert.deepEqual(skillBody("a", "b", "", undefined).tools, []);
});

test("snippets: Claude Code gets hooks and a bridge line, and the placeholder token until a real one is shown", () => {
  const withTok = snippets("http://dot:3335", "claude-code", "claude-code", "abc123");
  assert.equal(withTok[0]![0], "Claude Code: add to ~/.claude/settings.json");
  assert.ok(withTok[0]![1].includes('"url": "http://dot:3335/hook/claude-code"'));
  assert.ok(withTok[0]![1].includes("Bearer abc123"));
  JSON.parse(withTok[0]![1]); // the hooks snippet is valid JSON
  assert.ok(withTok.some(([t, s]) => t.startsWith("Let your voice replies") && s.includes("CLAWDOT_TOKEN=abc123") && s.includes("--endpoint claude-code")));
  const plain = snippets("http://dot:3335", "ci", "generic", undefined);
  assert.deepEqual(plain.map(([t]) => t), ["Try it from a terminal", "GitHub Actions: add as the last step of a job"]);
  assert.ok(plain[0]![1].includes("Bearer YOUR_TOKEN"));
  assert.ok(plain[1]![1].includes("${{ secrets.CLAWDOT_TOKEN }}"));
});

const settings: Settings = { fields: { device_name: { value: "Kitchen", source: "saved" }, zone: { value: null, source: "default" }, max_requests_per_day: { value: 100, source: "file" }, voice: { value: "x", source: "file" } } };
const specs: SettingSpec[] = [{ key: "device_name", kind: "text" }, { key: "zone", kind: "zone" }, { key: "max_requests_per_day", kind: "number" }, { key: "voice", kind: "text", off: true }];

test("settings patch sends only what changed", () => {
  const cur = Object.fromEntries(specs.map((s) => [s.key, currentText(settings, s.key)]));
  assert.deepEqual(cur, { device_name: "Kitchen", zone: "", max_requests_per_day: "100", voice: "x" });
  assert.deepEqual(settingsPatch(specs, cur, cur), {});
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, device_name: " Den ", max_requests_per_day: "250" }), { device_name: "Den", max_requests_per_day: 250 });
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, device_name: "" }), { device_name: null });
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, zone: "UTC" }), { zone: "UTC" });
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, voice: "other" }), {}, "a disabled field is never sent");
});

test("labels", () => {
  assert.equal(labelOf("llm_key_env"), "Key variable");
  assert.equal(labelOf("unknown_key"), "unknown_key");
  assert.equal(restartText(["listen", "public_url"]), "address this page uses, address agents use");
});
