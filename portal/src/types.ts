// Shapes of the JSON the device sends. Only the fields the portal reads are listed.

export type AuthMode = "setup" | "password" | "open";

export interface AuthState {
  mode: AuthMode;
  version: string;
  legacy?: boolean;
  open_allowed?: boolean;
  peer_private?: boolean;
  /** Seconds the local reset window has left (0 when closed). */
  reset_window_secs?: number;
}

/** What the speaker is doing, when the device says (older cores do not). */
export type SpeakerState = "idle" | "listening" | "thinking" | "speaking" | "muted";
export interface Status {
  clock_valid?: boolean;
  satellites?: number;
  llm?: { configured?: boolean; model?: string | null };
  usage?: {
    requests_today: number;
    max_requests_per_day: number;
    spend_today: number;
    /** The local day number today is, and the requests each recent day made. */
    day?: number;
    history?: { day: number; requests: number }[];
  };
  /** REQ-CFG-070. A missing field means unknown, not false. */
  speaker?: { connected?: boolean; available?: boolean; name?: string; state?: SpeakerState };
  voice?: { configured?: boolean };
  muted?: boolean;
  quiet_hours?: { set?: boolean; active?: boolean };
}

/** Why the device would not speak a test phrase (REQ-CFG-070). */
export type SpeakRefusal = "no_voice" | "no_speaker" | "busy" | "muted" | "quiet_hours";

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
  voice_info?: { language?: string; stt_model?: string; tts_model?: string; provider?: string | null };
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

/** What `GET /api/update` says (REQ-UPD-005). */
export interface UpdateRelease {
  version: string;
  notes: string;
  page: string;
  published: string;
}
export interface UpdateStatus {
  current: string;
  enabled: boolean;
  source: string;
  checking: boolean;
  last_checked: number | null;
  error: string | null;
  available: boolean;
  latest: UpdateRelease | null;
  can_apply: boolean;
  cannot_apply_because: string | null;
  supervised: boolean;
  /** Empty when idle; otherwise downloading, verifying, installing, installed, restarting or failed. */
  phase: string;
  apply_error: string | null;
  installed: string | null;
}
