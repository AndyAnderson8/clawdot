// A stand-in for the device: serves the built page with the real Content-Security-Policy and answers the JSON API
// with the same shapes the Rust core uses (crates/core/src/ui.rs), keeping state in memory.
import { readFileSync } from "node:fs";
import http from "node:http";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// Keep in step with `CSP` in crates/core/src/ui.rs (the smoke test checks the page works under it).
export const CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
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
const now = () => Math.floor(Date.now() / 1000);

function seed(mode: string): Json {
  const t = now();
  return {
    mode,
    password: "",
    session: "",
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
      listen: { value: "0.0.0.0:3335", source: "flag", live: false, ignored: true },
      public_url: { value: null, source: "default", live: true },
      max_requests_per_day: { value: 100, source: "default", live: true },
      daily_spend_usd: { value: null, source: "default", live: true },
      board: { value: "none", source: "default", live: false },
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
      },
      {
        name: "set_light",
        description: "Turn a light on or off",
        enabled: true,
        confirm: true,
        source: "http",
        origin: "portal",
        deletable: true,
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
    voice_info: { language: "en", stt_model: "whisper", tts_model: "voice-1" },
    speech_configured: false,
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
            llm: { configured: true },
            usage: { requests_today: 3, max_requests_per_day: 100, spend_today: 0.0123 },
          },
        });
      if (m === "GET" && path === "/api/overview")
        return send(200, { now: now(), endpoints: st["endpoints"], events: st["events"] });
      if (m === "POST" && path === "/api/speak")
        return body.text ? send(202, { queued: true }) : err(400, "give some text");
      if (m === "GET" && path === "/api/about")
        return send(200, { version: "0.1.0-test", changelog: "## [0.1.0]\n- First release" });
      if (m === "GET" && path === "/api/settings") return send(200, settingsJson());
      if (m === "PUT" && path === "/api/settings") {
        if (body.llm_url === "http://insecure.example.com")
          return err(400, "The language model address must start with https:// unless it is on your own network.");
        for (const [k, v] of Object.entries(body))
          st["settings"][k] = { value: v, source: v === null ? "default" : "saved", live: true };
        return send(200, settingsJson("Saved."));
      }
      if (m === "POST" && path === "/api/admin/password") {
        st["mode"] = "password";
        st["password"] = body.password;
        st["session"] = "sess-pw";
        return send(200, { mode: "password", token: st["session"] });
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
