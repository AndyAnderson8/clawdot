import assert from "node:assert/strict";
import { test } from "node:test";
import {
  cents,
  connectionDefinition,
  currentText,
  labelOf,
  matching,
  readSkillFiles,
  parseSettingsFile,
  reminderBody,
  restartText,
  settingsChanges,
  settingsPatch,
  skillSendProblem,
  skillImports,
  shownValue,
  skillBody,
  toolDefinition,
  type ReminderForm,
  type SettingSpec,
} from "../../src/forms.ts";
import { sampleSettings } from "./sample.ts";

const base: ReminderForm = {
  kind: "timer",
  label: "  tea ",
  amount: "5",
  unit: "60",
  time: "07:00",
  days: [],
  date: "",
  time2: "",
  when: "time",
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

// REQ-TMR-009
test("a reminder after a delay sends seconds and no date", () => {
  const f: ReminderForm = { ...base, kind: "reminder", when: "delay", amount: "10", unit: "60", date: "2026-12-01" };
  assert.deepEqual(reminderBody(f), { kind: "reminder", label: "tea", seconds: 600 });
  assert.deepEqual(reminderBody({ ...f, amount: "2", unit: "3600" }), {
    kind: "reminder",
    label: "tea",
    seconds: 7200,
  });
  assert.equal(reminderBody({ ...f, amount: "" }), "Enter how long to wait before the reminder.");
  assert.equal(reminderBody({ ...f, amount: "0" }), "Enter how long to wait before the reminder.");
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
    "Give each detail a name, or remove it.",
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

const settings = sampleSettings({
  device_name: ["Kitchen", "saved"],
  zone: [null, "default"],
  daily_spend_usd: [100, "file"],
  voice: ["x", "file"],
});
const specs: SettingSpec[] = [
  { key: "device_name" },
  { key: "zone" },
  { key: "daily_spend_usd" },
  { key: "voice", off: true },
];

test("settings patch sends only what changed", () => {
  const cur = Object.fromEntries(specs.map((s) => [s.key, currentText(settings, s.key)]));
  assert.deepEqual(cur, { device_name: "Kitchen", zone: "", daily_spend_usd: "100", voice: "x" });
  assert.deepEqual(settingsPatch(settings.fields, specs, cur, cur), {});
  assert.deepEqual(
    settingsPatch(settings.fields, specs, cur, { ...cur, device_name: " Den ", daily_spend_usd: "250" }),
    {
      device_name: "Den",
      daily_spend_usd: 250,
    },
  );
  assert.deepEqual(settingsPatch(settings.fields, specs, cur, { ...cur, device_name: "" }), { device_name: null });
  assert.deepEqual(settingsPatch(settings.fields, specs, cur, { ...cur, zone: "UTC" }), { zone: "UTC" });
  assert.deepEqual(
    settingsPatch(settings.fields, specs, cur, { ...cur, voice: "other" }),
    {},
    "a disabled field is never sent",
  );
});

test("a skill import the device would refuse is caught before it is sent", () => {
  const file = (text: string) => ({ path: "a/" + text.length, text });
  const one = (n: number) => Array.from({ length: n }, (_, i) => file("x".repeat(i + 1)));
  assert.equal(skillSendProblem(one(64), []), "");
  assert.match(skillSendProblem(one(65), []), /at most 64/);
  const big = [file("y".repeat(200 * 1024)), file("z".repeat(200 * 1024))];
  assert.match(skillSendProblem(big, []), /at most 384 KB/);
  const tools = Array.from({ length: 17 }, (_, i) => "t" + i);
  assert.match(skillSendProblem([], [{ name: "Many", tools }]), /more than 16 tools/);
  assert.equal(skillSendProblem([], [{ name: "Few", tools: tools.slice(0, 16) }]), "");
});

test("a number that is not a number is refused, not sent as null (which would reset the setting)", () => {
  const cur = Object.fromEntries(specs.map((s) => [s.key, currentText(settings, s.key)]));
  const got = settingsPatch(settings.fields, specs, cur, { ...cur, daily_spend_usd: "." });
  assert.equal(typeof got, "string");
  assert.match(typeof got === "string" ? got : "", /needs a number/);
  assert.equal(typeof settingsPatch(settings.fields, specs, cur, { ...cur, daily_spend_usd: "x1" }), "string");
  assert.deepEqual(settingsPatch(settings.fields, specs, cur, { ...cur, daily_spend_usd: "0.50" }), {
    daily_spend_usd: 0.5,
  });
});

test("labels", () => {
  assert.equal(labelOf("llm_key_env"), "Key variable");
  assert.equal(labelOf("unknown_key"), "unknown_key");
  assert.equal(restartText(["listen", "public_url"]), "portal address, address other programs use");
});

// REQ-CFG-062
test("agent settings: toggles become real booleans, times stay text, and an emptied box goes back to the default", () => {
  const agent: SettingSpec[] = [
    { key: "plan_speech" },
    { key: "dry_run" },
    { key: "agent_max_rounds" },
    { key: "quiet_start" },
    { key: "quiet_end" },
  ];
  const s = sampleSettings({
    plan_speech: [true, "default"],
    dry_run: [false, "default"],
    agent_max_rounds: [8, "default"],
    quiet_start: [null, "default"],
    quiet_end: [null, "default"],
  });
  const cur = Object.fromEntries(agent.map((a) => [a.key, currentText(s, a.key)]));
  assert.deepEqual(cur, {
    plan_speech: "true",
    dry_run: "false",
    agent_max_rounds: "8",
    quiet_start: "",
    quiet_end: "",
  });
  assert.deepEqual(settingsPatch(s.fields, agent, cur, cur), {});
  assert.deepEqual(
    settingsPatch(s.fields, agent, cur, {
      ...cur,
      plan_speech: "false",
      dry_run: "true",
      agent_max_rounds: "5",
      quiet_start: "22:00",
      quiet_end: "07:00",
    }),
    { plan_speech: false, dry_run: true, agent_max_rounds: 5, quiet_start: "22:00", quiet_end: "07:00" },
  );
  assert.deepEqual(settingsPatch(s.fields, agent, cur, { ...cur, plan_speech: "" }), { plan_speech: null });
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
  const notOurs = "That is not a ClawDot settings file. Use a file made by “Download backup”.";
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
  const s = sampleSettings({
    device_name: ["Kitchen", "default"],
    dry_run: [false, "default"],
    quiet_start: [null, "default"],
  });
  assert.deepEqual(settingsChanges(s, { device_name: "Kitchen", dry_run: true, quiet_start: "22:00", nothing: null }), [
    { key: "dry_run", label: "Dry run", from: "off", to: "on" },
    { key: "quiet_start", label: "Quiet from", from: "not set", to: "22:00" },
  ]);
  assert.deepEqual(settingsChanges(s, { device_name: "Kitchen" }), []);
  assert.equal(shownValue(""), "not set");
  assert.equal(shownValue(3), "3");
});

test("skills are read from Markdown files and skills files; what does not fit is listed with the reason", () => {
  const r = readSkillFiles([
    {
      path: "skills/tidy/SKILL.md",
      text: "---\nname: Tidy up\ndescription: Put things away\n---\nAsk which room first.\n",
    },
    { path: "skills/night/SKILL.md", text: "Turn off the lights." },
    { path: "skills/README.md", text: "About this folder" },
    { path: "skills/long.md", text: "x".repeat(2001) },
    {
      path: "skills/mine.json",
      text: JSON.stringify({ skills: [{ name: "Saved", instructions: "Do it.", tools: ["a"] }] }),
    },
    { path: "skills/broken.json", text: "not json" },
  ]);
  assert.deepEqual(
    r.found.map((s) => s.name),
    ["Tidy up", "night", "Saved"],
    "a header name wins, then the folder name",
  );
  assert.equal(r.found[0]?.instructions, "Put things away\n\nAsk which room first.");
  assert.deepEqual(r.found[2]?.tools, ["a"]);
  assert.equal(r.skipped.length, 2);
  assert.ok(r.skipped.some((s) => s.includes("long.md") && s.includes("2001")));
  assert.ok(r.skipped.some((s) => s.includes("broken.json")));
});

test("a search keeps rows that contain every word, in the name or the text", () => {
  const rows = [
    { name: "Good night", t: "turn off the lights" },
    { name: "Morning", t: "say the weather" },
  ];
  const find = (q: string) => matching(rows, q, (r) => r.t).map((r) => r.name);
  assert.deepEqual(find(""), ["Good night", "Morning"]);
  assert.deepEqual(find("NIGHT"), ["Good night"]);
  assert.deepEqual(find("the weather"), ["Morning"]);
  assert.deepEqual(find("night weather"), []);
});

test("a dollar amount keeps digits and a point, with at most two decimals", () => {
  assert.equal(cents("12"), "12");
  assert.equal(cents("12.5"), "12.5");
  assert.equal(cents("12.567"), "12.56");
  assert.equal(cents("$ 1,2.3.4"), "12.34");
  assert.equal(cents("abc"), "");
  assert.equal(cents("007.50"), "7.50");
  assert.equal(cents(""), "");
});

test("a skill folder is sent as its own files, and a loose file as its text", () => {
  const all = [
    { path: "s/tea/SKILL.md", text: "x" },
    { path: "s/tea/notes.txt", text: "y" },
    { path: "s/tea/inner/SKILL.md", text: "z" },
    { path: "s/loose.md", text: "w" },
  ];
  const out = skillImports(
    [
      { name: "Tea", instructions: "x", tools: [], path: "s/tea/SKILL.md" },
      { name: "loose", instructions: "w", tools: [], path: "s/loose.md" },
    ],
    all,
  );
  assert.deepEqual(
    out.folders.map((f) => f.path),
    ["s/tea/SKILL.md", "s/tea/notes.txt"],
    "its folder only, not a skill folder inside it",
  );
  assert.deepEqual(out.plain, [{ name: "loose", instructions: "w", tools: [] }]);
});
