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

export interface ParamForm { id: number; name: string; type: string; required: boolean; description: string }
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
  const def: ToolDefinition = { name: f.name.trim(), description: f.description.trim(), method: f.method, url: f.url.trim(), confirm: f.confirm, params: {} };
  if (f.tokenVar.trim()) def.auth_env = f.tokenVar.trim();
  for (const p of f.params) {
    const name = p.name.trim();
    if (!name) return "Give each value a name, or remove it.";
    const d = p.description.trim();
    def.params[name] = { type: p.type, required: p.required, ...(d ? { description: d } : {}) };
  }
  return def;
}

export interface ConnectionForm { name: string; url: string; tokenVar: string; confirm: boolean }
export interface ConnectionDefinition { name: string; url: string; confirm: boolean; auth_env?: string }

export function connectionDefinition(f: ConnectionForm): ConnectionDefinition {
  const d: ConnectionDefinition = { name: f.name.trim(), url: f.url.trim(), confirm: f.confirm };
  if (f.tokenVar.trim()) d.auth_env = f.tokenVar.trim();
  return d;
}

export function skillBody(name: string, instructions: string, tools: string, enabled: boolean | undefined) {
  return { name: name.trim(), instructions, tools: tools.split(/[\s,]+/).filter(Boolean), enabled };
}

/** Ready-to-paste setup text for one webhook. The token is the real one only right after it was made. */
export function snippets(base: string, name: string, adapter: string, tok: string | undefined): [string, string][] {
  const url = base + "/hook/" + name;
  const t = tok || "YOUR_TOKEN";
  const out: [string, string][] = [
    ["Try it from a terminal", "curl -X POST " + url + " \\\n  -H 'Authorization: Bearer " + t + "' \\\n  -d '{\"title\":\"Backup finished\",\"summary\":\"12 GB copied\"}'"],
  ];
  if (adapter === "claude-code") {
    const hook = (ev: string) => '    "' + ev + '": [{ "hooks": [{ "type": "http", "url": "' + url + '", "headers": { "Authorization": "Bearer ' + t + '" }, "timeout": 5 }] }]';
    out.unshift(["Claude Code: add to ~/.claude/settings.json", '{\n  "hooks": {\n' + hook("Stop") + ",\n" + hook("Notification") + "\n  }\n}"]);
    out.push(["Let your voice replies reach Claude Code (run on the computer where it works)", "CLAWDOT_TOKEN=" + t + " clawdot-bridge --url " + base + " --endpoint " + name]);
  }
  out.push(["GitHub Actions: add as the last step of a job", "- name: Tell ClawDot\n  if: always()\n  run: |\n    curl -fsS -X POST " + url + " \\\n      -H \"Authorization: Bearer ${{ secrets.CLAWDOT_TOKEN }}\" \\\n      -d '{\"title\":\"Build ${{ job.status }}\",\"summary\":\"${{ github.repository }}\"}'"]);
  return out;
}

export const LABEL: Record<string, string> = {
  device_name: "Device name", zone: "Time zone", persona: "Personality", llm_url: "Language model address", llm_model: "Model",
  llm_fallback_model: "Backup model", summary_model: "Summary model", llm_key_env: "Key variable", voice: "Voice",
  listen: "Address this page uses", public_url: "Address agents use", max_requests_per_day: "Requests per day",
  daily_spend_usd: "Daily spend limit", board: "Board",
};
export const FROM: Record<string, string> = { default: "Default", file: "Configuration file", saved: "Saved here", flag: "Command-line flag" };

export type SettingKind = "text" | "zone" | "area" | "board" | "number";
export interface SettingSpec { key: string; kind: SettingKind; hint?: string; off?: boolean }

export function labelOf(key: string): string { return LABEL[key] ?? key; }

/** The current value of a setting as the text a form field starts with. */
export function currentText(s: Settings, key: string): string {
  const v = s.fields[key]?.value;
  return v == null ? "" : String(v);
}

/** Only the values that changed, ready for `PUT /api/settings`; an emptied box becomes null (back to the default). */
export function settingsPatch(specs: SettingSpec[], current: Record<string, string>, edited: Record<string, string>): Record<string, string | number | null> {
  const body: Record<string, string | number | null> = {};
  for (const sp of specs) {
    if (sp.off) continue;
    const was = (current[sp.key] ?? "").trim();
    const v = (edited[sp.key] ?? "").trim();
    if (v === was) continue;
    body[sp.key] = v === "" ? null : sp.kind === "number" ? Number(v) : v;
  }
  return body;
}

export function restartText(keys: string[]): string {
  return keys.map((k) => labelOf(k).toLowerCase()).join(", ");
}
