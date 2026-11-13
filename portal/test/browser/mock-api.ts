// A stand-in for the device: serves the built page with the real Content-Security-Policy and answers the JSON API
// with the same shapes the Rust core uses (crates/core/src/ui.rs), keeping state in memory.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import http from "node:http";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const PASSWORD = "correct horse battery";

export interface Logged {
  method: string;
  path: string;
  body: unknown;
  auth: string;
}
// The fake device keeps whatever JSON the page sends, so its bodies and state are deliberately untyped. eslint.config.mjs
// relaxes the no-explicit-any and no-unsafe-* rules for this one file.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = Record<string, any>;

export interface Mock {
  url: string;
  log: Logged[];
  state: Record<string, unknown>;
  reset(mode: "setup" | "password" | "open"): void;
  close(): void;
}

const pageFile = join(
  fileURLToPath(new URL(".", import.meta.url)),
  "..",
  "..",
  "..",
  "crates",
  "core",
  "src",
  "ui.html",
);

/** The `'sha256-...'` sources of every inline `<tag>...</tag>` block of the page (what crates/core/build.rs computes). */
function inlineHashes(page: string, tag: string): string {
  const hashes = new Set<string>();
  const re = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`, "g");
  for (const m of page.matchAll(re)) hashes.add(`'sha256-${createHash("sha256").update(m[1]!).digest("base64")}'`);
  if (!hashes.size) throw new Error(`the built page has no inline <${tag}>`);
  return [...hashes].join(" ");
}

// The real Content-Security-Policy of crates/core/src/ui.rs: no 'unsafe-inline', the page's own inline script and
// style allowed by hash (computed here from the built page the same way build.rs does, so the browser suite proves the
// page works under it). Keep the rest in step with build.rs.
const pageHtml = readFileSync(pageFile, "utf8");
export const CSP =
  `default-src 'none'; script-src ${inlineHashes(pageHtml, "script")}; style-src ${inlineHashes(pageHtml, "style")}; ` +
  "connect-src 'self'; img-src data: https: http:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const now = () => Math.floor(Date.now() / 1000);

// What a setting goes back to when it is cleared.
const DEFAULTS: Json = { plan_speech: true, progress_speech: true, agent_max_rounds: 8 };

function auditSeed(t: number): Json[] {
  const rec = (id: number, o: Json): Json => ({
    v: 1,
    id,
    ts: t - (100 - id) * 60,
    day: 1,
    turn: id,
    trigger: "voice",
    args: {},
    ...o,
  });
  return [
    rec(1, {
      tool: "set_timer",
      tier: "reversible",
      decision: "ran",
      status: "ok",
      result: "Timer set for 5 minutes.",
      args: { seconds: 300, label: "tea" },
    }),
    rec(2, {
      tool: "set_light",
      tier: "costly",
      decision: "held",
      args: { room: "<b>kitchen</b>", token: "[redacted]" },
    }),
    rec(3, { tool: "set_light", tier: "costly", decision: "approved", held_id: 2, status: "ok", result: "ok" }),
    rec(4, { tool: "set_light", tier: "costly", decision: "declined", held_id: 2 }),
    rec(5, { tool: "home_toggle", tier: "costly", decision: "blocked:cap", args: { id: 7 }, note: "daily cap" }),
    rec(6, {
      tool: "github_prs",
      tier: "costly",
      trigger: "event:claude-code",
      decision: "proposed",
      args: { repo: "a/b" },
    }),
    rec(7, {
      tool: "fetch_page",
      tier: "costly",
      decision: "ran",
      status: "ok",
      result: "<script>alert(3)</script> long page",
      detail: "LONGER <img src=x onerror=alert(4)> text ".repeat(20),
    }),
  ];
}

function seed(mode: string): Json {
  const t = now();
  return {
    mode,
    password: "",
    session: "",
    window: 0,
    settings: {
      device_name: { value: "Kitchen Dot", source: "saved", live: true },
      zone: { value: "UTC", source: "file", live: true },
      persona: { value: "You are brief.", source: "default", live: true },
      llm_url: { value: "https://api.example.com/v1", source: "file", live: true },
      llm_model: { value: "model-a", source: "file", live: true },
      llm_key_env: { value: "API_KEY", source: "file", live: true },
      voice: { value: null, source: "default", live: true },
      speech_language: { value: null, source: "default", live: true },
      speech_provider: { value: "openai", source: "default", live: false },
      realtime_enabled: { value: false, source: "default", live: true },
      realtime_url: { value: null, source: "default", live: true },
      realtime_model: { value: null, source: "default", live: true },
      realtime_voice: { value: "alloy", source: "default", live: true },
      realtime_usd_per_min_in: { value: 0.03, source: "default", live: true },
      realtime_usd_per_min_out: { value: 0.1, source: "default", live: true },
      realtime_max_session_secs: { value: 300, source: "default", live: true },
      listen: { value: "0.0.0.0:3335", source: "flag", live: false, ignored: true },
      public_url: { value: null, source: "default", live: true },
      daily_spend_usd: { value: null, source: "default", live: true },
      llm_usd_per_mtok_in: { value: null, source: "default", live: true },
      llm_usd_per_mtok_out: { value: null, source: "default", live: true },
      agent_max_rounds: { value: 8, source: "default", live: true },
      agent_max_tool_calls: { value: 12, source: "file", live: true },
      agent_deadline_secs: { value: 60, source: "default", live: true },
      agent_tool_timeout_secs: { value: 45, source: "default", live: true },
      plan_speech: { value: true, source: "default", live: true },
      progress_speech: { value: true, source: "default", live: true },
      notes: { value: false, source: "default", live: true },
      update_check: { value: true, source: "default", live: true },
      max_actions_per_day: { value: 100, source: "default", live: true },
      max_costly_per_day: { value: 20, source: "default", live: true },
      dry_run: { value: true, source: "flag", live: true, saved: false },
      quiet_start: { value: null, source: "default", live: true },
      quiet_end: { value: null, source: "default", live: true },
    },
    restart: ["listen"],
    schedule: [
      { id: 1, kind: "timer", label: "pasta", fire_at: t + 600, seconds_left: 600, repeat: null },
      {
        id: 2,
        kind: "alarm",
        label: "<b>work</b>",
        fire_at: t + 7200,
        seconds_left: 7200,
        repeat: { hour: 7, minute: 30, days: [0, 1, 2, 3, 4], every_day: false },
      },
    ],
    nextId: 3,
    endpoints: [{ name: "claude-code", adapter: "claude-code", enabled: true, created_at: t - 100 }],
    events: [
      {
        id: 1,
        endpoint: "claude-code",
        source: "claude-code",
        title: "<img src=x onerror=alert(1)>Build done",
        text: "<script>alert(2)</script> all green",
        state: "new",
        received_at: t - 120,
      },
    ],
    tools: [
      {
        name: "set_timer",
        description: "Start a timer",
        enabled: true,
        confirm: false,
        source: "builtin",
        deletable: false,
        tier: "reversible",
        tier_default: "reversible",
        tier_set: false,
        tier_options: ["reversible", "costly"],
      },
      {
        name: "set_light",
        description: "Turn a light on or off",
        enabled: true,
        confirm: true,
        source: "http",
        origin: "portal",
        deletable: true,
        tier: "costly",
        tier_default: "costly",
        tier_set: false,
        tier_options: ["read_only", "reversible", "costly"],
      },
      {
        name: "home_toggle",
        description: "Toggle a switch",
        enabled: true,
        confirm: true,
        source: "mcp",
        server: "home",
        deletable: false,
      },
    ],
    mcp: [
      {
        name: "home",
        url: "https://ha.local/mcp",
        enabled: true,
        connected: true,
        deletable: true,
        origin: "portal",
        token_env: "HA_TOKEN",
        tools: [
          {
            name: "home_toggle",
            description: "Toggle a switch",
            enabled: true,
            confirm: true,
            source: "mcp",
            server: "home",
          },
        ],
      },
    ],
    audit: auditSeed(t),
    logs: [{ ts: t - 30, level: "error", text: "could not save the notes: <b>disk full</b>" }],
    update: {
      current: "0.1.0-test",
      enabled: true,
      source: "api.github.com",
      checking: false,
      last_checked: t - 3600,
      error: null,
      available: false,
      latest: null,
      can_apply: false,
      cannot_apply_because: null,
      supervised: true,
      phase: "",
      apply_error: null,
      installed: null,
    },
    notesOn: true,
    speechOn: false,
    keyHint: "sk-or-…c9f2",
    notes: [{ id: 1, text: "my printer is <b>Pixel</b>", ts: t - 3600 }],
    nextNote: 2,
    skills: [{ name: "Good night", instructions: "Turn off the lights.", tools: ["set_light"], enabled: true }],
    problems: [],
  };
}

/**
 * Every API route the fake device answers, written as in docs/api/openapi.yaml (`{name}` for a path parameter).
 * Keep it next to the handlers in startMock: test/unit/mock-routes.test.ts fails when a route here is not in the
 * description, or when a handler below matches an exact path that is not listed here.
 */
export const MOCK_ROUTES: readonly { method: string; path: string }[] = [
  { method: "GET", path: "/api/auth" },
  { method: "POST", path: "/api/setup" },
  { method: "POST", path: "/api/login" },
  { method: "POST", path: "/api/recover/window" },
  { method: "POST", path: "/api/logout" },
  { method: "GET", path: "/api/status" },
  { method: "GET", path: "/api/overview" },
  { method: "POST", path: "/api/speak" },
  { method: "GET", path: "/api/about" },
  { method: "GET", path: "/api/models" },
  { method: "POST", path: "/api/models" },
  { method: "PUT", path: "/api/llm/key" },
  { method: "DELETE", path: "/api/llm/key" },
  { method: "PUT", path: "/api/speech/key" },
  { method: "DELETE", path: "/api/speech/key" },
  { method: "PUT", path: "/api/realtime/key" },
  { method: "DELETE", path: "/api/realtime/key" },
  { method: "GET", path: "/api/hive/peers" },
  { method: "GET", path: "/api/voices" },
  { method: "GET", path: "/api/update" },
  { method: "POST", path: "/api/update/check" },
  { method: "POST", path: "/api/update/apply" },
  { method: "GET", path: "/api/logs" },
  { method: "GET", path: "/api/diagnostics" },
  { method: "GET", path: "/api/settings" },
  { method: "PUT", path: "/api/settings" },
  { method: "GET", path: "/api/settings/export" },
  { method: "POST", path: "/api/admin/password" },
  { method: "GET", path: "/api/audit" },
  { method: "GET", path: "/api/notes" },
  { method: "POST", path: "/api/notes" },
  { method: "DELETE", path: "/api/notes" },
  { method: "PUT", path: "/api/notes/{id}" },
  { method: "DELETE", path: "/api/notes/{id}" },
  { method: "GET", path: "/api/schedule" },
  { method: "POST", path: "/api/schedule" },
  { method: "PUT", path: "/api/schedule/{id}" },
  { method: "DELETE", path: "/api/schedule/{id}" },
  { method: "POST", path: "/api/endpoints" },
  { method: "PUT", path: "/api/endpoints/{name}" },
  { method: "DELETE", path: "/api/endpoints/{name}" },
  { method: "POST", path: "/api/endpoints/{name}/rotate" },
  { method: "GET", path: "/api/tools" },
  { method: "POST", path: "/api/tools" },
  { method: "DELETE", path: "/api/tools/{name}" },
  { method: "POST", path: "/api/tools/{name}/enabled" },
  { method: "POST", path: "/api/tools/{name}/tier" },
  { method: "POST", path: "/api/connections" },
  { method: "POST", path: "/api/connections/test" },
  { method: "DELETE", path: "/api/connections/{name}" },
  { method: "POST", path: "/api/connections/{name}/enabled" },
  { method: "POST", path: "/api/connections/{name}/accept" },
  { method: "GET", path: "/api/skills" },
  { method: "POST", path: "/api/skills" },
  { method: "POST", path: "/api/skills/{action}" },
];

export async function startMock(initial: "setup" | "password" | "open" = "open"): Promise<Mock> {
  let st = seed(initial);
  const log: Logged[] = [];
  const mock: Mock = {
    url: "",
    log,
    get state() {
      return st;
    },
    reset(mode) {
      st = seed(mode);
      log.length = 0;
    },
    close() {
      server.close();
    },
  };

  const ext = () => ({ tools: st["tools"], mcp: st["mcp"], skills: st["skills"], problems: st["problems"] });
  const sched = () => ({ now: now(), clock_valid: true, items: st["schedule"] });
  const settingsJson = (note?: string) => ({
    fields: st["settings"],
    restart_needed: st["restart"],
    llm: { key_present: !!st["keyHint"], key_hint: st["keyHint"] || null },
    speech: { key_present: !!st["speechKey"], key_hint: st["speechKey"] || null },
    realtime: {
      configured: !!st["realtimeKey"],
      built: st["realtimeBuilt"] !== false,
      provider: "openai",
      key_hint: st["realtimeKey"] || null,
    },
    voice_info: { language: "en", stt_model: "whisper", tts_model: "voice-1", provider: "speech.example" },
    speech_configured: !!st["speechOn"],
    speakers_listen: "0.0.0.0:3336",
    ...(note ? { note } : {}),
  });

  const server = http.createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const path = (req.url ?? "").split("?")[0]!;
      const send = (code: number, v: unknown, headers: Record<string, string> = {}) => {
        res.writeHead(code, { "content-type": "application/json", "cache-control": "no-store", ...headers });
        res.end(JSON.stringify(v));
      };
      if (req.method === "GET" && (path === "/" || path === "/ui")) {
        res.writeHead(200, {
          "content-type": "text/html; charset=utf-8",
          "content-security-policy": CSP,
          "x-content-type-options": "nosniff",
          "cache-control": "no-store",
        });
        res.end(readFileSync(pageFile));
        return;
      }
      if (!path.startsWith("/api/")) return send(404, { error: "not found" });
      const m = req.method ?? "GET";
      const body = raw ? JSON.parse(raw) : null;
      const auth = String(req.headers["authorization"] ?? "");
      log.push({ method: m, path, body, auth });
      const seg = path.slice(5).split("/");
      const err = (code: number, message: string) => send(code, { error: message });

      // Without sign-in
      if (m === "GET" && path === "/api/auth")
        return send(200, {
          mode: st["mode"],
          version: "0.1.0-test",
          legacy: false,
          open_allowed: true,
          peer_private: true,
          reset_window_secs: st["window"],
        });
      if (m === "POST" && path === "/api/setup") {
        if (st["mode"] !== "setup") return err(409, "already set up");
        if (body.mode === "password") {
          st["mode"] = "password";
          st["password"] = body.password;
          st["session"] = "sess-setup";
          return send(200, { mode: "password", token: st["session"] });
        }
        st["mode"] = "open";
        return send(200, { mode: "open" });
      }
      if (m === "POST" && path === "/api/login") {
        if (body.password !== st["password"] && body.password !== PASSWORD) return err(401, "Wrong password.");
        st["session"] = "sess-login";
        return send(200, { token: st["session"] });
      }
      if (m === "POST" && path === "/api/recover/window") {
        if (!st["window"]) return err(403, "the reset window is not open");
        st["window"] = 0;
        st["password"] = body.password;
        st["session"] = "sess-window";
        return send(200, { mode: "password", token: st["session"] });
      }
      if (m === "POST" && path === "/api/logout") {
        st["session"] = "";
        return send(200, { signed_out: true });
      }
      // Everything else needs the administrator.
      if (st["mode"] === "setup") return err(403, "Finish setup first.");
      if (st["mode"] === "password" && (!st["session"] || auth !== "Bearer " + st["session"]))
        return send(401, { error: "Sign in first." }, { "www-authenticate": "Bearer" });

      if (m === "GET" && path === "/api/status")
        return send(200, {
          now: now(),
          status: {
            hardware: {
              board: st["hwNoLabel"] ? "unknown" : "test-board",
              label: st["hwNoLabel"] ? "" : "Test Speaker",
              model: "Test Model 9",
              cpu: "TestSoC",
              cores: 4,
              ram_mb: 486,
              arch: "arm",
              os: "linux",
              parts: { leds: st["hwLeds"] !== false, bluetooth: false, wifi: true, screen: false, usb_serial: false },
              configured_parts: st["hwLeds"] !== false ? ["leds"] : [],
            },
            clock_valid: true,
            satellites: 1,
            llm: { configured: true, model: "test-model" },
            usage: {
              requests_today: 3,
              spend_today: 0.0123,
              spend_basis: st["spendBasis"] ?? "reported",
              day: Math.floor(Date.now() / 86400000),
              history: [{ day: Math.floor(Date.now() / 86400000) - 1, requests: 5 }],
            },
            speaker: { connected: true, available: true },
            voice: { configured: true },
            muted: false,
            quiet_hours: { set: false, active: false },
            ...(st["statusExtra"] as Json | undefined),
          },
        });
      if (m === "GET" && path === "/api/overview")
        return send(200, { now: now(), endpoints: st["endpoints"], events: st["events"] });
      if (m === "POST" && path === "/api/speak") {
        const why = st["speakRefusal"] as Json | undefined;
        if (why)
          return send(why["status"], {
            ok: false,
            reason: why["reason"],
            message: why["message"],
            error: why["message"],
          });
        return body.text ? send(202, { ok: true, queued: true }) : err(400, "give some text");
      }
      if (m === "GET" && path === "/api/about")
        return send(200, {
          version: "0.1.0-test",
          build: { version: "0.1.0-test", profile: "release", os: "linux", arch: "aarch64", features: ["tz", "tls"] },
          changelog: "## [0.1.0]\n- First release",
        });
      if (m === "GET" && path === "/api/models") return send(200, { models: ["model-b", "model-a", "free/model-c"] });
      if (m === "POST" && path === "/api/models") {
        const key = typeof body.key === "string" ? body.key : "";
        const ok = key ? key.startsWith("sk-") : !!st["keyHint"];
        if (!ok)
          return err(
            401,
            "The service did not accept that key. Check that it is the whole key and that it is for this service.",
          );
        return send(200, { models: ["model-b", "model-a", "free/model-c"] });
      }
      if (m === "PUT" && path === "/api/llm/key") {
        const k = String(body.key ?? "");
        st["keyHint"] = k.slice(0, 6) + "…" + k.slice(-4);
        return send(200, { hint: st["keyHint"] });
      }
      for (const [route, slot] of [
        ["/api/speech/key", "speechKey"],
        ["/api/realtime/key", "realtimeKey"],
      ] as const) {
        if (m === "PUT" && path === route) {
          const k = String(body.key ?? "");
          if (/\s/.test(k)) return err(400, "A key is one line of letters, digits and dashes, with no spaces.");
          st[slot] = k.slice(0, 6) + "…" + k.slice(-4);
          return send(200, { hint: st[slot] });
        }
        if (m === "DELETE" && path === route) {
          st[slot] = "";
          return send(200, { hint: null });
        }
      }
      if (m === "GET" && path === "/api/hive/peers") {
        if (!st["hive"]) return err(404, "not found");
        return send(200, {
          discovery: "native",
          self: { id: "5e56829b0991be4b6be71bba3f639cac", name: "Kitchen Dot", port: 3335 },
          peers: st["hive"],
        });
      }
      if (m === "DELETE" && path === "/api/llm/key") {
        st["keyHint"] = "";
        return send(200, { hint: null });
      }
      if (m === "GET" && path === "/api/voices") {
        if (!st["speechOn"]) return err(503, "No speech provider is set up, so there are no voices to choose from.");
        return send(200, {
          voices: ["alloy", "nova", "onyx"],
          details: [
            { id: "alloy", label: "alloy", category: null },
            { id: "nova", label: "Nova (my voice)", category: "cloned" },
            { id: "onyx", label: "onyx", category: null },
          ],
          source: "builtin",
        });
      }
      if (m === "GET" && path === "/api/update") return send(200, st["update"]);
      if (m === "POST" && path === "/api/update/check") {
        const u = st["update"] as Json;
        u["last_checked"] = now();
        return send(200, u);
      }
      if (m === "POST" && path === "/api/update/apply") {
        const u = st["update"] as Json;
        if (!u["latest"] || body.version !== u["latest"].version)
          return err(409, "That version is not a newer one than this. Check for updates again.");
        if (!u["can_apply"]) return err(409, String(u["cannot_apply_because"]));
        u["phase"] = "installing";
        return send(202, u);
      }
      if (m === "GET" && path === "/api/logs")
        return send(200, {
          lines: st["logs"],
          max_lines: 200,
        });
      if (m === "GET" && path === "/api/diagnostics")
        return send(200, { format: "clawdot-diagnostics", version: "0.1.0-test", endpoints: st["endpoints"] });
      if (m === "GET" && path === "/api/settings/export") {
        const saved: Json = {};
        for (const [k, f] of Object.entries(st["settings"] as Json)) if (f["source"] === "saved") saved[k] = f["value"];
        return send(200, { format: "clawdot-settings", version: 1, settings: saved });
      }
      if (m === "GET" && path === "/api/settings") return send(200, settingsJson());
      if (m === "PUT" && path === "/api/settings") {
        if (body.llm_url === "http://insecure.example.com")
          return err(400, "The language model address must start with https:// unless it is on your own network.");
        const ranges: Record<string, [number, number, string]> = {
          agent_max_rounds: [1, 20, "Rounds per request"],
          max_actions_per_day: [0, 100000, "Actions per day"],
        };
        for (const [k, v] of Object.entries(body)) {
          const r = ranges[k];
          if (r && v !== null && !(Number.isInteger(v) && (v as number) >= r[0] && (v as number) <= r[1]))
            return err(400, `${r[2]} must be a whole number from ${r[0]} to ${r[1]}.`);
        }
        if (
          ("quiet_start" in body || "quiet_end" in body) &&
          !(body.quiet_start && body.quiet_end) &&
          !(body.quiet_start === null && body.quiet_end === null)
        )
          return err(400, "Set both the quiet hours start and end, or clear both.");
        for (const [k, v] of Object.entries(body)) {
          const was = st["settings"][k] ?? {};
          st["settings"][k] = {
            ...was,
            value: v === null ? (DEFAULTS[k] ?? null) : v,
            source: was.source === "flag" ? "flag" : v === null ? "default" : "saved",
            saved: v !== null,
            ignored: was.source === "flag",
            live: true,
          };
        }
        return send(200, settingsJson("Saved."));
      }
      if (m === "POST" && path === "/api/admin/password") {
        // With a password in force the current one is needed; a wrong one is a 403, which does not sign the page out.
        if (st["mode"] === "password" && body.current !== st["password"] && body.current !== PASSWORD)
          return err(403, "the current password is missing or wrong");
        st["mode"] = "password";
        st["password"] = body.password;
        st["session"] = "sess-pw";
        return send(200, { mode: "password", token: st["session"] });
      }

      if (seg[0] === "notes") {
        const notes = st["notes"] as Json[];
        const list = () => ({
          enabled: st["notesOn"],
          notes,
          limits: { max_notes: 50, max_note_chars: 200, max_total_bytes: 8192 },
        });
        if (m === "GET") {
          const q = (new URL(req.url ?? "", "http://x").searchParams.get("q") ?? "").toLowerCase().trim();
          if (!q) return send(200, list());
          const words = q.split(/\s+/);
          return send(200, {
            ...list(),
            query: q,
            notes: notes
              .map((n, i): Json => ({ ...n, position: i + 1 }))
              .filter((n) => words.some((w) => String(n["text"]).toLowerCase().includes(w))),
          });
        }
        if (m === "POST") {
          if (!st["notesOn"]) return err(409, "Memory is turned off. Turn it on in Settings, under Features.");
          if (/sk-/.test(body.text)) return err(400, "That looks like a password or a key, so it is not saved.");
          if (notes.some((n) => n["text"].toLowerCase() === body.text.toLowerCase()))
            return err(409, "That is already remembered.");
          notes.push({ id: st["nextNote"]++, text: body.text, ts: now() });
          return send(200, list());
        }
        if (m === "PUT" && seg[1]) {
          const n = notes.find((x) => String(x["id"]) === seg[1]);
          if (!n) return err(404, "There is no such memory.");
          if (notes.some((x) => x !== n && x["text"].toLowerCase() === String(body.text).toLowerCase()))
            return err(409, "That is already remembered.");
          n["text"] = body.text;
          return send(200, list());
        }
        if (m === "DELETE" && seg[1]) {
          if (!notes.some((n) => String(n["id"]) === seg[1])) return err(404, "no such memory");
          st["notes"] = notes.filter((n) => String(n["id"]) !== seg[1]);
          return send(200, { forgotten: 1 });
        }
        if (m === "DELETE") {
          const n = notes.length;
          st["notes"] = [];
          return send(200, { forgotten: n });
        }
      }

      if (m === "GET" && path === "/api/audit") {
        const q = new URL(req.url ?? "", "http://x").searchParams;
        const limit = Number(q.get("limit") ?? 50);
        const before = q.has("before") ? Number(q.get("before")) : Infinity;
        const hit = (st["audit"] as Json[])
          .filter(
            (r) =>
              r["id"] < before &&
              (!q.get("tool") || r["tool"] === q.get("tool")) &&
              (!q.get("tier") || r["tier"] === q.get("tier")) &&
              (!q.get("decision") ||
                (q.get("decision")!.endsWith(":")
                  ? r["decision"].startsWith(q.get("decision")!)
                  : r["decision"] === q.get("decision"))) &&
              (!q.get("trigger") ||
                r["trigger"] === q.get("trigger") ||
                r["trigger"].startsWith(q.get("trigger") + ":")),
          )
          .sort((a, b) => b["id"] - a["id"]);
        const records = hit.slice(0, limit);
        return send(200, {
          records,
          next_before: hit.length > limit ? records[records.length - 1]!["id"] : null,
          log: { bytes: 2048, max_file_bytes: 262144, corrupt_lines: 0, write_errors: 0 },
          policy: { dry_run: false, max_actions_per_day: 100, max_costly_per_day: 20 },
        });
      }

      if (seg[0] === "schedule") {
        const id = seg[1] ? Number(seg[1]) : 0;
        const list = st["schedule"] as Json[];
        if (m === "GET") return send(200, sched());
        if (m === "DELETE") {
          if (!list.some((i) => i["id"] === id)) return err(404, "no such timer, alarm, or reminder");
          st["schedule"] = list.filter((i) => i["id"] !== id);
          return send(200, sched());
        }
        if (body.kind === "timer" && !(body.seconds >= 1)) return err(400, "Timers run from 1 second to 24 hours.");
        if (body.kind === "reminder" && body.seconds !== undefined) {
          if (!body.label) return err(400, "A reminder needs some text to say.");
          if (!(body.seconds >= 1)) return err(400, "That time does not work.");
          if (body.date || body.time) return err(400, "Give a reminder either a delay or a date and a time, not both.");
        }
        const item = {
          id: id || st["nextId"]++,
          kind: body.kind,
          label: body.label,
          fire_at: now() + (body.seconds ?? 3600),
          seconds_left: body.seconds ?? 3600,
          countdown: body.kind === "timer" || (body.kind === "reminder" && body.seconds !== undefined),
          repeat:
            body.kind === "alarm" && body.days.length
              ? { hour: body.hour, minute: body.minute, days: body.days, every_day: body.days.length === 7 }
              : null,
        };
        st["schedule"] = id ? list.map((i) => (i["id"] === id ? item : i)) : [...list, item];
        return send(200, sched());
      }

      if (seg[0] === "endpoints") {
        const eps = st["endpoints"] as Json[];
        if (m === "POST" && seg.length === 1) {
          if (eps.some((e) => e["name"] === body.name)) return err(409, "That name is taken.");
          const spoken = body.spoken_name ?? null;
          eps.push({ name: body.name, adapter: body.adapter, enabled: true, created_at: now(), spoken_name: spoken });
          return send(201, {
            name: body.name,
            adapter: body.adapter,
            spoken_name: spoken,
            token: "tok-" + body.name + "-1",
          });
        }
        if (m === "PUT" && seg.length === 2) {
          const e = eps.find((x) => x["name"] === seg[1]);
          if (!e) return err(404, "no such endpoint");
          const spoken = String(body.spoken_name ?? "").trim();
          if (spoken && !/^[\p{L}\p{N}][\p{L}\p{N} \-_'.]{0,39}$/u.test(spoken)) return err(400, "bad spoken name");
          e["spoken_name"] = spoken || null;
          return send(200, e);
        }
        if (m === "POST" && seg[2] === "rotate") return send(200, { name: seg[1], token: "tok-" + seg[1] + "-2" });
        if (m === "DELETE") {
          st["endpoints"] = eps.filter((e) => e["name"] !== seg[1]);
          return send(200, { deleted: seg[1] });
        }
      }

      if (seg[0] === "tools" || seg[0] === "skills" || seg[0] === "connections") {
        if (m === "GET" && seg[0] !== "connections") return send(200, ext()); // the real device lists connections with the tools
        const tools = st["tools"] as Json[];
        if (seg[0] === "tools") {
          if (m === "POST" && seg.length === 1) {
            tools.push({
              name: body.name,
              description: body.description,
              enabled: true,
              confirm: body.confirm,
              source: "http",
              origin: "portal",
              deletable: true,
            });
            return send(200, ext());
          }
          if (m === "DELETE") {
            st["tools"] = tools.filter((t) => t["name"] !== seg[1]);
            return send(200, ext());
          }
          if (seg[2] === "enabled") {
            for (const t of tools) if (t["name"] === seg[1]) t["enabled"] = body.enabled;
            return send(200, ext());
          }
          if (seg[2] === "tier") {
            const t = tools.find((x) => x["name"] === seg[1]);
            if (!t) return err(404, "There is no such tool.");
            if (body.tier === "destructive") return err(400, "Destructive cannot be chosen here.");
            t["tier_set"] = body.tier !== null;
            t["tier"] = body.tier ?? t["tier_default"];
            t["confirm"] = t["tier"] === "costly";
            return send(200, ext());
          }
        }
        if (seg[0] === "connections") {
          const mcp = st["mcp"] as Json[];
          if (seg[1] === "test") return send(200, { tools: [{ name: "a_tool" }, { name: "b_tool" }] });
          if (m === "POST" && seg.length === 1) {
            mcp.push({
              name: body.name,
              url: body.url,
              enabled: true,
              connected: true,
              deletable: true,
              origin: "portal",
              tools: [],
            });
            return send(200, ext());
          }
          if (m === "DELETE") {
            st["mcp"] = mcp.filter((s) => s["name"] !== seg[1]);
            return send(200, ext());
          }
          if (m === "POST" && seg[2] === "accept") {
            const src = mcp.find((x) => x["name"] === seg[1]);
            if (!src) return err(404, "There is no such connection.");
            const waiting = (src["needs_review"] ?? []) as Json[];
            const taken = body.tool ? waiting.filter((t) => t["name"] === body.tool) : waiting;
            if (body.tool && !taken.length) return err(404, "There is no such tool waiting.");
            src["needs_review"] = waiting.filter((t) => !taken.includes(t));
            for (const t of taken)
              (src["tools"] as Json[]).push({
                name: t["name"],
                description: t["description"],
                enabled: true,
                confirm: true,
                source: "mcp",
                server: src["name"],
              });
            return send(200, ext());
          }
          if (seg[2] === "enabled") {
            for (const s of mcp) if (s["name"] === seg[1]) s["enabled"] = body.enabled;
            return send(200, ext());
          }
        }
        if (seg[0] === "skills" && m === "POST") {
          const skills = st["skills"] as Json[];
          if (seg.length === 1) {
            skills.push({ name: body.name, instructions: body.instructions, tools: body.tools, enabled: body.enabled });
            return send(200, ext());
          }
          if (seg[1] === "edit") {
            st["skills"] = skills.map((k) => (k["name"] === body.name ? { ...body.skill, name: body.skill.name } : k));
            return send(200, ext());
          }
          if (seg[1] === "delete") {
            st["skills"] = skills.filter((k) => k["name"] !== body.name);
            return send(200, ext());
          }
          if (seg[1] === "enabled") {
            const k = skills.find((x) => x["name"] === body.name);
            if (k && body.enabled && k["needs_review"])
              return err(
                400,
                "This skill was imported and has not been reviewed. Open it, read it, and choose Accept before turning it on.",
              );
            if (k) k["enabled"] = body.enabled;
            return send(200, ext());
          }
          if (seg[1] === "accept") {
            const k = skills.find((x) => x["name"] === body.name);
            if (k) k["needs_review"] = false;
            return send(200, ext());
          }
          if (seg[1] === "import") {
            // Like the device: every imported skill is off and waits for review; a folder is read from its raw files.
            const skipped: string[] = [];
            let added = 0;
            if (body.format === "skill-folder") {
              const files = (body.files ?? []) as { path: string; text: string }[];
              for (const f of files.filter((x) => /(^|\/)SKILL\.md$/i.test(x.path))) {
                const dir = f.path.slice(0, f.path.lastIndexOf("/") + 1);
                const head = /^---\n([\s\S]*?)\n---\n?([\s\S]*)$/.exec(f.text);
                const get = (k: string) => new RegExp("^" + k + ":\\s*(.*)$", "m").exec(head?.[1] ?? "")?.[1] ?? "";
                skills.push({
                  name: get("name") || dir.replace(/\/$/, ""),
                  description: get("description"),
                  instructions: (head?.[2] ?? f.text).trim(),
                  tools: [],
                  enabled: false,
                  needs_review: true,
                  source: f.path,
                  files: files
                    .filter((x) => x !== f && x.path.startsWith(dir))
                    .map((x) => ({ path: x.path.slice(dir.length), text: x.text })),
                });
                added++;
              }
            } else {
              for (const k of body.skills ?? []) {
                skills.push({ ...k, enabled: false, needs_review: true });
                added++;
              }
            }
            return send(200, { ...ext(), import: { added, skipped } });
          }
        }
      }
      return err(404, "not found");
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  mock.url = `http://127.0.0.1:${(server.address() as { port: number }).port}`;
  return mock;
}
