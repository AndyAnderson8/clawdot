import assert from "node:assert/strict";
import { test } from "node:test";
import {
  connectionDefinition,
  currentText,
  labelOf,
  parseSettingsFile,
  reminderBody,
  restartText,
  settingsChanges,
  settingsPatch,
  shownValue,
  skillBody,
  toolDefinition,
  type ReminderForm,
  type SettingSpec,
} from "../../src/forms.ts";
import type { Settings } from "../../src/types.ts";

const base: ReminderForm = {
  kind: "timer",
  label: "  tea ",
  amount: "5",
  unit: "60",
  time: "07:00",
  days: [],
  date: "",
  time2: "",
};

test("timer body is seconds, trimmed label", () => {
  assert.deepEqual(reminderBody(base), { kind: "timer", label: "tea", seconds: 300 });
  assert.deepEqual(reminderBody({ ...base, amount: "1.5", unit: "3600" }), {
    kind: "timer",
    label: "tea",
    seconds: 5400,
  });
});

test("alarm body has hour, minute, and the chosen days", () => {
  assert.deepEqual(reminderBody({ ...base, kind: "alarm", time: "06:30", days: [0, 2] }), {
    kind: "alarm",
    label: "tea",
    hour: 6,
    minute: 30,
    days: [0, 2],
  });
  assert.equal(reminderBody({ ...base, kind: "alarm", time: "" }), "Pick a time for the alarm.");
});

test("reminder body needs a date and a time", () => {
  assert.deepEqual(reminderBody({ ...base, kind: "reminder", date: "2026-12-01", time2: "09:15" }), {
    kind: "reminder",
    label: "tea",
    date: "2026-12-01",
    time: "09:15",
  });
  assert.equal(
    reminderBody({ ...base, kind: "reminder", date: "", time2: "09:15" }),
    "Pick a date and a time for the reminder.",
  );
  assert.equal(
    reminderBody({ ...base, kind: "reminder", date: "2026-12-01", time2: "" }),
    "Pick a date and a time for the reminder.",
  );
});

test("tool definition", () => {
  const f = {
    name: " set_light ",
    description: " Turn a light on ",
    method: "POST",
    url: " https://h/{room} ",
    tokenVar: " HA_TOKEN ",
    confirm: true,
    params: [
      { id: 1, name: "room", type: "string", required: true, description: "" },
      { id: 2, name: "on", type: "boolean", required: false, description: " yes or no " },
    ],
  };
  assert.deepEqual(toolDefinition(f), {
    name: "set_light",
    description: "Turn a light on",
    method: "POST",
    url: "https://h/{room}",
    confirm: true,
    auth_env: "HA_TOKEN",
    params: {
      room: { type: "string", required: true },
      on: { type: "boolean", required: false, description: "yes or no" },
    },
  });
  assert.equal(
    toolDefinition({ ...f, params: [{ id: 3, name: " ", type: "string", required: true, description: "" }] }),
    "Give each value a name, or remove it.",
  );
  assert.equal("auth_env" in (toolDefinition({ ...f, tokenVar: " " }) as object), false);
});

test("connection definition", () => {
  assert.deepEqual(connectionDefinition({ name: " home ", url: " https://h/mcp ", tokenVar: "", confirm: false }), {
    name: "home",
    url: "https://h/mcp",
    confirm: false,
  });
  assert.deepEqual(connectionDefinition({ name: "home", url: "u", tokenVar: "T", confirm: true }), {
    name: "home",
    url: "u",
    confirm: true,
    auth_env: "T",
  });
});

test("skill body splits tools on commas and spaces", () => {
  assert.deepEqual(skillBody(" Good night ", "Turn off lights", "set_light, set_alarm  x", true), {
    name: "Good night",
    instructions: "Turn off lights",
    tools: ["set_light", "set_alarm", "x"],
    enabled: true,
  });
  assert.deepEqual(skillBody("a", "b", "", undefined).tools, []);
});

// (the snippets are tested in snippets.test.ts)

const settings: Settings = {
  fields: {
    device_name: { value: "Kitchen", source: "saved" },
    zone: { value: null, source: "default" },
    max_requests_per_day: { value: 100, source: "file" },
    voice: { value: "x", source: "file" },
  },
};
const specs: SettingSpec[] = [
  { key: "device_name", kind: "text" },
  { key: "zone", kind: "zone" },
  { key: "max_requests_per_day", kind: "number" },
  { key: "voice", kind: "text", off: true },
];

test("settings patch sends only what changed", () => {
  const cur = Object.fromEntries(specs.map((s) => [s.key, currentText(settings, s.key)]));
  assert.deepEqual(cur, { device_name: "Kitchen", zone: "", max_requests_per_day: "100", voice: "x" });
  assert.deepEqual(settingsPatch(specs, cur, cur), {});
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, device_name: " Den ", max_requests_per_day: "250" }), {
    device_name: "Den",
    max_requests_per_day: 250,
  });
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, device_name: "" }), { device_name: null });
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, zone: "UTC" }), { zone: "UTC" });
  assert.deepEqual(settingsPatch(specs, cur, { ...cur, voice: "other" }), {}, "a disabled field is never sent");
});

test("labels", () => {
  assert.equal(labelOf("llm_key_env"), "Key variable");
  assert.equal(labelOf("unknown_key"), "unknown_key");
  assert.equal(restartText(["listen", "public_url"]), "address this page uses, address agents use");
});

// REQ-CFG-062
test("agent settings: toggles become real booleans, times stay text, and an emptied box goes back to the default", () => {
  const agent: SettingSpec[] = [
    { key: "plan_speech", kind: "bool" },
    { key: "dry_run", kind: "bool" },
    { key: "agent_max_rounds", kind: "number", min: 1, max: 20 },
    { key: "quiet_start", kind: "time" },
    { key: "quiet_end", kind: "time" },
  ];
  const s: Settings = {
    fields: {
      plan_speech: { value: true, source: "default" },
      dry_run: { value: false, source: "default" },
      agent_max_rounds: { value: 8, source: "default" },
      quiet_start: { value: null, source: "default" },
      quiet_end: { value: null, source: "default" },
    },
  };
  const cur = Object.fromEntries(agent.map((a) => [a.key, currentText(s, a.key)]));
  assert.deepEqual(cur, {
    plan_speech: "true",
    dry_run: "false",
    agent_max_rounds: "8",
    quiet_start: "",
    quiet_end: "",
  });
  assert.deepEqual(settingsPatch(agent, cur, cur), {});
  assert.deepEqual(
    settingsPatch(agent, cur, {
      ...cur,
      plan_speech: "false",
      dry_run: "true",
      agent_max_rounds: "5",
      quiet_start: "22:00",
      quiet_end: "07:00",
    }),
    { plan_speech: false, dry_run: true, agent_max_rounds: 5, quiet_start: "22:00", quiet_end: "07:00" },
  );
  assert.deepEqual(settingsPatch(agent, cur, { ...cur, plan_speech: "" }), { plan_speech: null });
  assert.equal(labelOf("quiet_start"), "Quiet from");
});

// REQ-CFG-065
test("a settings file is checked before anything is sent", () => {
  const file = (o: unknown) => JSON.stringify(o);
  const good = { format: "clawdot-settings", version: 1, settings: { device_name: "Den", dry_run: true, zone: null } };
  assert.deepEqual(parseSettingsFile(file(good)), { settings: { device_name: "Den", dry_run: true, zone: null } });
  const msg = (t: string): string => {
    const r = parseSettingsFile(t);
    return typeof r === "string" ? r : "loaded";
  };
  const notOurs = "That is not a ClawDot settings file. Use a file made by “Save my settings”.";
  for (const bad of [
    "{nope",
    "[]",
    "5",
    file({ format: "other", version: 1, settings: {} }),
    file({ format: "clawdot-settings", version: 1 }),
  ]) {
    assert.equal(parseSettingsFile(bad), notOurs, bad);
  }
  assert.match(msg(file({ ...good, version: 2 })), /different version/);
  assert.match(msg(file({ ...good, settings: { persona: { a: 1 } } })), /persona/);
  const many = Object.fromEntries(Array.from({ length: 65 }, (_, i) => ["k" + i, i]));
  assert.match(msg(file({ ...good, settings: many })), /too many/);
});

// REQ-CFG-065
test("loading a file lists only what would change, in plain words", () => {
  const s: Settings = {
    fields: {
      device_name: { value: "Kitchen", source: "default" },
      dry_run: { value: false, source: "default" },
      quiet_start: { value: null, source: "default" },
    },
  };
  assert.deepEqual(settingsChanges(s, { device_name: "Kitchen", dry_run: true, quiet_start: "22:00", nothing: null }), [
    { key: "dry_run", label: "Dry run", from: "off", to: "on" },
    { key: "quiet_start", label: "Quiet from", from: "not set", to: "22:00" },
  ]);
  assert.deepEqual(settingsChanges(s, { device_name: "Kitchen" }), []);
  assert.equal(shownValue(""), "not set");
  assert.equal(shownValue(3), "3");
});
