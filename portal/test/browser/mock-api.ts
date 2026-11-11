// A stand-in for the device: serves the built page with the real Content-Security-Policy and answers the JSON API
// with the same shapes the Rust core uses (crates/core/src/ui.rs), keeping state in memory.
import { readFileSync } from "node:fs";
import http from "node:http";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// Keep in step with `CSP` in crates/core/src/ui.rs (the smoke test checks the page works under it).
export const CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src data: https: http:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
export const PASSWORD = "correct horse battery";
/** The recovery code the mock device holds in password mode. */
export const RECOVERY = "ABCDE-FGHJK-MNPQR-STUVW";
const NEXT_RECOVERY = "WXYZ2-34567-89ABC-DEFGH";

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
    recovery: mode === "password" ? RECOVERY : "",
    window: 0,
    settings: {
      device_name: { value: "Kitchen Dot", source: "saved", live: true },
      zone: { value: "UTC", source: "file", live: true },
      persona: { value: "You are brief.", source: "default", live: true },
      llm_url: { value: "https://api.example.com/v1", source: "file", live: true },
      llm_model: { value: "model-a", source: "file", live: true },
      llm_fallback_model: { value: null, source: "default", live: true },
      summary_model: { value: null, source: "default", live: true },
      llm_key_env: { value: "API_KEY", source: "file", live: true },
      voice: { value: null, source: "default", live: true },
      speech_language: { value: null, source: "default", live: true },
      listen: { value: "0.0.0.0:3335", source: "flag", live: false, ignored: true },
      public_url: { value: null, source: "default", live: true },
      max_requests_per_day: { value: 100, source: "default", live: true },
      daily_spend_usd: { value: null, source: "default", live: true },
      board: { value: "none", source: "default", live: false },
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
    notes: [{ id: 1, text: "my printer is <b>Pixel</b>", ts: t - 3600 }],
    nextNote: 2,
    skills: [{ name: "Good night", instructions: "Turn off the lights.", tools: ["set_light"], enabled: true }],
    problems: [],
  };
}

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
    llm: { key_present: true },
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
          recovery: !!st["recovery"],
          reset_window_secs: st["window"],
        });
      if (m === "POST" && path === "/api/setup") {
        if (st["mode"] !== "setup") return err(409, "already set up");
        if (body.mode === "password") {
          st["mode"] = "password";
          st["password"] = body.password;
          st["session"] = "sess-setup";
          st["recovery"] = RECOVERY;
          return send(200, { mode: "password", token: st["session"], recovery_code: RECOVERY });
        }
        st["mode"] = "open";
        return send(200, { mode: "open" });
      }
      if (m === "POST" && path === "/api/login") {
        if (body.password !== st["password"] && body.password !== PASSWORD) return err(401, "Wrong password.");
        st["session"] = "sess-login";
        return send(200, { token: st["session"] });
      }
      const plain = (c: unknown) =>
        String(c)
          .replace(/[^a-z0-9]/gi, "")
          .toUpperCase();
      if (m === "POST" && path === "/api/recover") {
        if (!st["recovery"] || plain(body.code) !== plain(st["recovery"]))
          return err(401, "That recovery code was not accepted. Check it and try again.");
        st["password"] = body.password;
        st["session"] = "sess-recover";
        st["recovery"] = NEXT_RECOVERY;
        return send(200, { mode: "password", token: st["session"], recovery_code: NEXT_RECOVERY });
      }
      if (m === "POST" && path === "/api/recover/window") {
        if (!st["window"]) return err(403, "the reset window is not open");
        st["window"] = 0;
        st["password"] = body.password;
        st["session"] = "sess-window";
        st["recovery"] = NEXT_RECOVERY;
        return send(200, { mode: "password", token: st["session"], recovery_code: NEXT_RECOVERY });
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
            clock_valid: true,
            satellites: 1,
            llm: { configured: true, model: "test-model" },
            usage: {
              requests_today: 3,
              max_requests_per_day: 100,
              spend_today: 0.0123,
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
      if (m === "GET" && path === "/api/voices") {
        if (!st["speechOn"]) return err(503, "No speech provider is set up, so there are no voices to choose from.");
        return send(200, { voices: ["alloy", "nova", "onyx"], source: "builtin" });
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
        const first = !st["recovery"];
        st["mode"] = "password";
        st["password"] = body.password;
        st["session"] = "sess-pw";
        if (first) st["recovery"] = RECOVERY;
        return send(200, { mode: "password", token: st["session"], ...(first ? { recovery_code: RECOVERY } : {}) });
      }
      if (m === "POST" && path === "/api/admin/recovery") {
        st["recovery"] = NEXT_RECOVERY;
        return send(200, { recovery_code: NEXT_RECOVERY });
      }

      if (seg[0] === "notes") {
        const notes = st["notes"] as Json[];
        const list = () => ({
          enabled: st["notesOn"],
          notes,
          limits: { max_notes: 50, max_note_chars: 200, max_total_bytes: 8192 },
        });
        if (m === "GET") return send(200, list());
        if (m === "POST") {
          if (!st["notesOn"]) return err(409, "Notes are turned off. Turn them on in Settings, under Agent settings.");
          if (/sk-/.test(body.text)) return err(400, "That looks like a password or a key, so it is not saved.");
          if (notes.some((n) => n["text"].toLowerCase() === body.text.toLowerCase()))
            return err(409, "That note is already saved.");
          notes.push({ id: st["nextNote"]++, text: body.text, ts: now() });
          return send(200, list());
        }
        if (m === "DELETE" && seg[1]) {
          if (!notes.some((n) => String(n["id"]) === seg[1])) return err(404, "no such note");
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
        const item = {
          id: id || st["nextId"]++,
          kind: body.kind,
          label: body.label,
          fire_at: now() + (body.seconds ?? 3600),
          seconds_left: body.seconds ?? 3600,
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
          eps.push({ name: body.name, adapter: body.adapter, enabled: true, created_at: now() });
          return send(201, { name: body.name, adapter: body.adapter, token: "tok-" + body.name + "-1" });
        }
        if (m === "POST" && seg[2] === "rotate") return send(200, { name: seg[1], token: "tok-" + seg[1] + "-2" });
        if (m === "DELETE") {
          st["endpoints"] = eps.filter((e) => e["name"] !== seg[1]);
          return send(200, { deleted: seg[1] });
        }
      }

      if (seg[0] === "tools" || seg[0] === "skills" || seg[0] === "connections") {
        if (m === "GET") return send(200, ext());
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
            for (const k of skills) if (k["name"] === body.name) k["enabled"] = body.enabled;
            return send(200, ext());
          }
          if (seg[1] === "import") {
            for (const k of body.skills ?? []) skills.push({ ...k, enabled: false });
            return send(200, ext());
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
