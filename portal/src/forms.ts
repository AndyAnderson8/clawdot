// Turning form state into request bodies, and the copy-and-paste snippets. Pure functions, unit-tested.
import type { ScheduleKind, Settings } from "./types.ts";

export interface ReminderForm {
  kind: ScheduleKind;
  label: string;
  amount: string;
  unit: string;
  time: string;
  days: number[];
  date: string;
  time2: string;
}

export interface ReminderBody {
  kind: ScheduleKind;
  label: string;
  seconds?: number;
  hour?: number;
  minute?: number;
  days?: number[];
  date?: string;
  time?: string;
}

/** The body to send, or the message to show when the form is incomplete. */
export function reminderBody(f: ReminderForm): ReminderBody | string {
  const body: ReminderBody = { kind: f.kind, label: f.label.trim() };
  if (f.kind === "timer") {
    body.seconds = Math.round(Number(f.amount) * Number(f.unit));
  } else if (f.kind === "alarm") {
    if (!f.time) return "Pick a time for the alarm.";
    const [h, m] = f.time.split(":");
    body.hour = Number(h);
    body.minute = Number(m);
    body.days = [...f.days];
  } else {
    if (!f.date || !f.time2) return "Pick a date and a time for the reminder.";
    body.date = f.date;
    body.time = f.time2;
  }
  return body;
}

export interface ParamForm {
  id: number;
  name: string;
  type: string;
  required: boolean;
  description: string;
}
export interface ToolForm {
  name: string;
  description: string;
  method: string;
  url: string;
  tokenVar: string;
  confirm: boolean;
  params: ParamForm[];
}
export interface ToolDefinition {
  name: string;
  description: string;
  method: string;
  url: string;
  confirm: boolean;
  params: Record<string, { type: string; required: boolean; description?: string }>;
  auth_env?: string;
}

export function toolDefinition(f: ToolForm): ToolDefinition | string {
  const def: ToolDefinition = {
    name: f.name.trim(),
    description: f.description.trim(),
    method: f.method,
    url: f.url.trim(),
    confirm: f.confirm,
    params: {},
  };
  if (f.tokenVar.trim()) def.auth_env = f.tokenVar.trim();
  for (const p of f.params) {
    const name = p.name.trim();
    if (!name) return "Give each detail a name, or remove it.";
    const d = p.description.trim();
    def.params[name] = { type: p.type, required: p.required, ...(d ? { description: d } : {}) };
  }
  return def;
}

export interface ConnectionForm {
  name: string;
  url: string;
  tokenVar: string;
  confirm: boolean;
}
export interface ConnectionDefinition {
  name: string;
  url: string;
  confirm: boolean;
  auth_env?: string;
}

export function connectionDefinition(f: ConnectionForm): ConnectionDefinition {
  const d: ConnectionDefinition = { name: f.name.trim(), url: f.url.trim(), confirm: f.confirm };
  if (f.tokenVar.trim()) d.auth_env = f.tokenVar.trim();
  return d;
}

export function skillBody(
  name: string,
  instructions: string,
  tools: string,
  enabled: boolean | undefined,
): { name: string; instructions: string; tools: string[]; enabled: boolean | undefined } {
  return { name: name.trim(), instructions, tools: tools.split(/[\s,]+/).filter(Boolean), enabled };
}

export const LABEL: Record<string, string> = {
  device_name: "Device name",
  zone: "Time zone",
  persona: "Instructions",
  llm_url: "Address",
  llm_model: "Model",
  speech_language: "Language you speak",
  llm_key_env: "Key variable",
  voice: "Voice",
  listen: "Portal address",
  public_url: "Address other programs use",
  daily_spend_usd: "Most to spend per day",
  board: "Board",
  agent_max_rounds: "Rounds per request",
  agent_max_tool_calls: "Tool calls per request",
  agent_deadline_secs: "Seconds per request",
  agent_tool_timeout_secs: "Seconds per tool call",
  plan_speech: "Say the plan first",
  progress_speech: "Say progress while working",
  notes: "Remember things",
  max_actions_per_day: "Actions per day",
  max_costly_per_day: "Actions that ask first, per day",
  dry_run: "Dry run",
  quiet_start: "Quiet from",
  quiet_end: "Quiet until",
};
export const FROM: Record<string, string> = {
  default: "Standard setting",
  file: "Preset on this device",
  saved: "You changed this here",
  flag: "Set when ClawDot was started",
};

export type SettingKind =
  "text" | "zone" | "area" | "board" | "number" | "money" | "bool" | "time" | "language" | "voice" | "model";
export interface SettingSpec {
  key: string;
  kind: SettingKind;
  hint?: string;
  off?: boolean;
  /** An optional setting: shown only once added, with this text on the button that adds it, and a Remove button. */
  optional?: string;
  /** Hide the control (its value is still kept and saved) while this returns true for the form's current values. */
  hideWhen?: (vals: Record<string, string>) => boolean;
  /** For a whole number: the range the device accepts (shown to the browser as min and max). */
  min?: number;
  max?: number;
}

export function labelOf(key: string): string {
  return LABEL[key] ?? key;
}

/** The current value of a setting as the text a form field starts with. */
export function currentText(s: Settings, key: string): string {
  const v = s.fields[key]?.value;
  return String(v ?? "");
}

/** Only the values that changed, ready for `PUT /api/settings`; an emptied box becomes null (back to the default). */
export function settingsPatch(
  specs: SettingSpec[],
  current: Record<string, string>,
  edited: Record<string, string>,
): Record<string, string | number | boolean | null> {
  const body: Record<string, string | number | boolean | null> = {};
  for (const sp of specs) {
    if (sp.off) continue;
    const was = (current[sp.key] ?? "").trim();
    const v = (edited[sp.key] ?? "").trim();
    if (v === was) continue;
    body[sp.key] =
      v === "" ? null : sp.kind === "number" || sp.kind === "money" ? Number(v) : sp.kind === "bool" ? v === "true" : v;
  }
  return body;
}

export function restartText(keys: string[]): string {
  return keys.map((k) => labelOf(k).toLowerCase()).join(", ");
}

/** A setting as a person reads it: on or off, "not set", or the text. */
export function shownValue(v: unknown): string {
  if (v === null || v === undefined || v === "") return "not set";
  if (typeof v === "boolean") return v ? "on" : "off";
  return typeof v === "string" || typeof v === "number" ? String(v) : JSON.stringify(v);
}

/** The settings in a file saved by "Download backup", or the plain reason it cannot be one. */
export function parseSettingsFile(
  text: string,
): { settings: Record<string, string | number | boolean | null> } | string {
  const notOurs = "That is not a ClawDot settings file. Use a file made by “Download backup”.";
  let doc: unknown;
  try {
    doc = JSON.parse(text);
  } catch {
    return notOurs;
  }
  if (typeof doc !== "object" || doc === null || Array.isArray(doc)) return notOurs;
  const d = doc as Record<string, unknown>;
  if (d["format"] !== "clawdot-settings") return notOurs;
  if (d["version"] !== 1) return "This file comes from a different version of ClawDot and cannot be loaded here.";
  const raw = d["settings"];
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return notOurs;
  const out: Record<string, string | number | boolean | null> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (!(v === null || typeof v === "string" || typeof v === "number" || typeof v === "boolean"))
      return "The value of “" + k + "” in the file is not something a setting can hold.";
    out[k] = v;
  }
  if (Object.keys(out).length > 64) return "That file has too many settings to be a ClawDot settings file.";
  return { settings: out };
}

export interface SettingChange {
  key: string;
  label: string;
  from: string;
  to: string;
}

/** What loading `incoming` would change, compared with the values in force now. */
export function settingsChanges(s: Settings, incoming: Record<string, unknown>): SettingChange[] {
  const out: SettingChange[] = [];
  for (const [key, to] of Object.entries(incoming)) {
    const now = s.fields[key]?.value ?? null;
    if (JSON.stringify(now) === JSON.stringify(to)) continue;
    out.push({ key, label: labelOf(key), from: shownValue(now), to: shownValue(to) });
  }
  return out;
}

// ----- skills found in files -----

export interface SkillDraft {
  name: string;
  instructions: string;
  tools: string[];
  /** The file it was read from. */
  path?: string;
}
export interface SkillFile {
  /** The path inside the chosen folder, or just the file name. */
  path: string;
  text: string;
}
export const SKILL_NAME_MAX = 40;
export const SKILL_TEXT_MAX = 2000;

/** Splits `---` header lines (name, description) from the text of a skill file. */
function skillHeader(text: string): { head: Record<string, string>; body: string } {
  const m = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/.exec(text);
  if (!m) return { head: {}, body: text };
  const head: Record<string, string> = {};
  for (const line of (m[1] ?? "").split(/\r?\n/)) {
    const kv = /^([A-Za-z_-]+):\s*(.*)$/.exec(line);
    if (kv) head[(kv[1] ?? "").toLowerCase()] = (kv[2] ?? "").replace(/^["']|["']$/g, "").trim();
  }
  return { head, body: m[2] ?? "" };
}

/**
 * Reads skills out of chosen files: a Markdown file per skill (a `SKILL.md` with a name and description at the top, the
 * name of its folder otherwise) or a skills file saved by ClawDot. Anything that does not fit is listed in `skipped`
 * with the reason, so nothing disappears silently.
 */
export function readSkillFiles(files: SkillFile[]): { found: SkillDraft[]; skipped: string[] } {
  const found: SkillDraft[] = [];
  const skipped: string[] = [];
  const add = (where: string, d: SkillDraft) => {
    const name = d.name.trim();
    if (!name) skipped.push(where + ": it has no name");
    else if (name.length > SKILL_NAME_MAX)
      skipped.push(where + ": the name is longer than " + SKILL_NAME_MAX + " characters");
    else if (!d.instructions.trim()) skipped.push(where + ": it has no instructions");
    else if (d.instructions.length > SKILL_TEXT_MAX)
      skipped.push(
        where + ": the instructions are " + d.instructions.length + " characters; the limit is " + SKILL_TEXT_MAX,
      );
    else if (found.some((f) => f.name.toLowerCase() === name.toLowerCase()))
      skipped.push(where + ": the name " + name + " is already in this list");
    else found.push({ name, instructions: d.instructions.trim(), tools: d.tools, path: where });
  };
  for (const f of files) {
    const parts = f.path.split("/");
    const file = parts[parts.length - 1] ?? f.path;
    if (/\.json$/i.test(file)) {
      let v: { skills?: { name?: unknown; instructions?: unknown; tools?: unknown }[] };
      try {
        v = JSON.parse(f.text) as typeof v;
      } catch {
        skipped.push(f.path + ": it is not a skills file");
        continue;
      }
      if (!Array.isArray(v.skills)) {
        skipped.push(f.path + ": it is not a skills file");
        continue;
      }
      for (const k of v.skills)
        add(f.path, {
          name: typeof k.name === "string" ? k.name : "",
          instructions: typeof k.instructions === "string" ? k.instructions : "",
          tools: Array.isArray(k.tools) ? k.tools.filter((t): t is string => typeof t === "string") : [],
        });
    } else if (/\.md$/i.test(file) && !/^readme/i.test(file)) {
      const { head, body } = skillHeader(f.text);
      const folder = file.toLowerCase() === "skill.md" ? (parts[parts.length - 2] ?? "") : file.replace(/\.md$/i, "");
      const text = [head["description"], body.trim()].filter(Boolean).join("\n\n");
      add(f.path, { name: head["name"] || folder, instructions: text, tools: [] });
    }
  }
  return { found, skipped };
}

/** Keeps the rows whose name or text contains every word typed (not case-sensitive). */
export function matching<T extends { name: string }>(rows: T[], query: string, text: (r: T) => string = () => ""): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return rows;
  return rows.filter((r) => {
    const hay = (r.name + " " + text(r)).toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** What was typed into a dollar amount, kept to digits and a point with at most two decimals ("12.5" stays, "12.567" becomes "12.56"). */
export function cents(typed: string): string {
  const t = typed.replace(/[^0-9.]/g, "");
  const [whole = "", ...rest] = t.split(".");
  if (!rest.length) return whole.replace(/^0+(?=\d)/, "");
  return whole.replace(/^0+(?=\d)/, "") + "." + rest.join("").slice(0, 2);
}

/** Plain-text files a skill folder may carry besides its SKILL.md. Anything else (scripts, programs) is never sent. */
export const SKILL_HELPER = /\.(md|txt|json|ya?ml|csv|tsv|toml)$/i;

/**
 * What to send for the chosen skills. A skill that is a folder with a `SKILL.md` is sent as its raw files (the device
 * reads the header itself and keeps the helper files); anything else (a single Markdown file, a ClawDot skills file) is
 * sent as the name and instructions already read here.
 */
export function skillImports(
  chosen: SkillDraft[],
  all: SkillFile[],
): { folders: SkillFile[]; plain: { name: string; instructions: string; tools: string[] }[] } {
  const folders: SkillFile[] = [];
  const plain: { name: string; instructions: string; tools: string[] }[] = [];
  for (const d of chosen) {
    const p = d.path ?? "";
    if (/(^|\/)SKILL\.md$/i.test(p) && p.includes("/")) {
      const dir = p.slice(0, p.lastIndexOf("/") + 1);
      // Only the skill's own folder, not folders inside it that hold another skill.
      for (const f of all) if (f.path.startsWith(dir) && !f.path.slice(dir.length).includes("/")) folders.push(f);
    } else plain.push({ name: d.name, instructions: d.instructions, tools: d.tools });
  }
  return { folders, plain };
}
