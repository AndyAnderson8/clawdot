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
    if (!name) return "Give each value a name, or remove it.";
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
  persona: "Personality",
  llm_url: "Language model address",
  llm_model: "Model",
  llm_fallback_model: "Backup model",
  summary_model: "Summary model",
  llm_key_env: "Key variable",
  voice: "Voice",
  listen: "Address this page uses",
  public_url: "Address agents use",
  max_requests_per_day: "Requests per day",
  daily_spend_usd: "Daily spend limit",
  board: "Board",
  agent_max_rounds: "Rounds per request",
  agent_max_tool_calls: "Tool calls per request",
  agent_deadline_secs: "Seconds per request",
  agent_tool_timeout_secs: "Seconds per tool call",
  plan_speech: "Say the plan first",
  progress_speech: "Say progress while working",
  notes: "Keep notes",
  max_actions_per_day: "Actions per day",
  max_costly_per_day: "Actions that ask first, per day",
  dry_run: "Dry run",
  quiet_start: "Quiet from",
  quiet_end: "Quiet until",
};
export const FROM: Record<string, string> = {
  default: "Default",
  file: "Configuration file",
  saved: "Saved here",
  flag: "Command-line flag",
};

export type SettingKind = "text" | "zone" | "area" | "board" | "number" | "bool" | "time";
export interface SettingSpec {
  key: string;
  kind: SettingKind;
  hint?: string;
  off?: boolean;
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
    body[sp.key] = v === "" ? null : sp.kind === "number" ? Number(v) : sp.kind === "bool" ? v === "true" : v;
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

/** The settings in a file saved by "Save my settings", or the plain reason it cannot be one. */
export function parseSettingsFile(
  text: string,
): { settings: Record<string, string | number | boolean | null> } | string {
  const notOurs = "That is not a ClawDot settings file. Use a file made by “Save my settings”.";
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
