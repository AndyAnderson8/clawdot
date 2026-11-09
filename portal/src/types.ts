// Shapes of the JSON the device sends. Only the fields the portal reads are listed.

export type AuthMode = "setup" | "password" | "open";

export interface AuthState {
  mode: AuthMode;
  version: string;
  legacy?: boolean;
  open_allowed?: boolean;
  peer_private?: boolean;
}

export interface Status {
  clock_valid?: boolean;
  satellites?: number;
  llm?: { configured?: boolean };
  usage?: { requests_today: number; max_requests_per_day: number; spend_today: number };
}

export type EventState = "new" | "announced" | "acknowledged" | "dismissed" | "snoozed";
export interface AgentEvent {
  id: number;
  endpoint: string;
  source: string;
  title: string;
  text: string;
  state: EventState | string;
  received_at: number;
}
export interface Endpoint {
  name: string;
  adapter: string;
  enabled: boolean;
  created_at: number;
}
export interface Overview {
  now: number;
  endpoints: Endpoint[];
  events: AgentEvent[];
}

export type ScheduleKind = "timer" | "alarm" | "reminder";
export interface Repeat {
  hour: number;
  minute: number;
  days: number[];
  every_day: boolean;
}
export interface ScheduleItem {
  id: number;
  kind: ScheduleKind;
  label: string;
  fire_at: number;
  seconds_left: number;
  repeat?: Repeat | null;
}
export interface ScheduleList {
  now: number;
  clock_valid: boolean;
  items: ScheduleItem[];
}

export interface Tool {
  name: string;
  description: string;
  enabled: boolean;
  confirm: boolean;
  source: "builtin" | "http" | "mcp" | string;
  origin?: string;
  server?: string;
  deletable?: boolean;
  /** The risk tier in force, the one it was declared with, whether the portal changed it, and what may be chosen. */
  tier?: string;
  tier_default?: string;
  tier_set?: boolean;
  tier_options?: string[];
}
export interface McpServer {
  name: string;
  url: string;
  enabled: boolean;
  connected: boolean;
  deletable?: boolean;
  origin?: string;
  error?: string;
  token_env?: string;
  tools: Tool[];
}
export interface Skill {
  name: string;
  instructions: string;
  tools?: string[];
  enabled: boolean;
}
export interface Problem {
  kind: string;
  name: string;
  reason: string;
}
export interface Extensions {
  tools?: Tool[];
  mcp?: McpServer[];
  skills?: Skill[];
  problems?: Problem[];
}

export interface SettingField {
  value: string | number | boolean | null;
  source: "default" | "file" | "saved" | "flag" | string;
  live?: boolean;
  saved?: boolean;
  ignored?: boolean;
}
export interface Settings {
  fields: Record<string, SettingField>;
  restart_needed?: string[];
  llm?: { key_present?: boolean };
  voice_info?: { language?: string; stt_model?: string; tts_model?: string };
  speech_configured?: boolean;
  speakers_listen?: string;
  note?: string;
}

/** One line of the audit log. Argument and result text can come from outside: show it as text only. */
export interface AuditRecord {
  id: number;
  ts: number;
  turn: number;
  trigger: string;
  tool: string;
  tier: string;
  args: unknown;
  decision: string;
  held_id?: number;
  dry_run?: boolean;
  status?: string;
  result?: string;
  detail?: string;
  turn_cost_usd?: number;
  note?: string;
}
export interface AuditPage {
  records: AuditRecord[];
  next_before: number | null;
  log: { bytes: number; max_file_bytes: number; corrupt_lines: number; write_errors: number };
  policy: { dry_run: boolean; max_actions_per_day: number; max_costly_per_day: number };
}

export interface Note {
  id: number;
  text: string;
  ts: number;
}
export interface NotesList {
  enabled: boolean;
  notes: Note[];
  limits: { max_notes: number; max_note_chars: number; max_total_bytes: number };
}

export interface WebhookToken {
  name: string;
  token: string;
}
export interface ConnectionTest {
  tools: { name: string }[];
}

export interface BuildInfo {
  version: string;
  profile: string;
  os: string;
  arch: string;
  features: string[];
}
export interface About {
  version: string;
  build?: BuildInfo;
  changelog: string;
}
export interface LogLine {
  ts: number;
  level: string;
  text: string;
}
export interface LogList {
  lines: LogLine[];
  max_lines: number;
}
