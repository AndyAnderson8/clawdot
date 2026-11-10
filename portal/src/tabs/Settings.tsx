import type { JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import { Fragment } from "preact";
import { api, ApiError, keep } from "../api.ts";
import { currentText, FROM, labelOf, restartText, settingsPatch, type SettingSpec } from "../forms.ts";
import { fail, get, say, set, signedOut, start, sv, useApp } from "../store.ts";
import { zoneName } from "../time.ts";
import { RecoverySettings, showCode } from "../recovery.tsx";
import { About } from "./About.tsx";
import type { Settings as SettingsData } from "../types.ts";

function browserZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch {
    return "";
  }
}
function zoneList(): string[] | null {
  try {
    const l = Intl.supportedValuesOf("timeZone");
    if (l.length) return ["UTC", ...l];
  } catch {
    /* an older browser: fall back to a text box */
  }
  return null;
}

/** One group of settings with its own Save button. Only the values you changed are sent. */
function Group(props: { title: string; intro?: string; specs: SettingSpec[]; settings: SettingsData }) {
  const { title, intro, specs, settings } = props;
  const current: Record<string, string> = {};
  const start0: Record<string, string> = {};
  const hints: Record<string, string> = {};
  const zones: string[] = [];
  const bz = browserZone();
  const zs = zoneList();
  for (const sp of specs) {
    const f = settings.fields[sp.key];
    if (!f) continue;
    const cur = currentText(settings, sp.key);
    current[sp.key] = cur;
    start0[sp.key] = cur;
    hints[sp.key] = sp.hint ?? "";
    if (sp.kind === "zone" && zs) {
      zones.push(...(zs.includes(cur) ? zs : [cur, ...zs]));
      if (f.source === "default" && bz && zs.includes(bz)) {
        start0[sp.key] = bz;
        hints[sp.key] =
          "Your browser says " +
          zoneName(bz) +
          ", so that is pre-selected. Save to use it. (This comes from your browser, never from your internet address.)";
      }
    }
  }
  const [vals, setVals] = useState(start0);
  const [err, setErr] = useState("");
  const errRef = useRef<HTMLParagraphElement>(null);

  const submit = async (ev: Event) => {
    ev.preventDefault();
    setErr("");
    const body = settingsPatch(specs, current, vals);
    if (!Object.keys(body).length) {
      say("Nothing to save: you have not changed anything here.");
      return;
    }
    try {
      const next = await api<SettingsData>("PUT", "/api/settings", body);
      const merged = Object.assign({}, get().settings, next);
      set({ settings: merged, settingsRev: get().settingsRev + 1 });
      say(merged.note || "Saved.");
    } catch (x) {
      if (x instanceof ApiError && x.status === 400) {
        setErr(x.message);
        errRef.current?.scrollIntoView({ block: "nearest" });
      } else fail(x);
    }
  };

  return (
    <form class="panel" onSubmit={submit}>
      <h3>{title}</h3>
      {intro ? <p class="mut">{intro}</p> : null}
      {specs.map((sp) => {
        const f = settings.fields[sp.key];
        if (!f) return null;
        const id = "f-" + sp.key;
        const v = vals[sp.key] ?? "";
        const input = (e: Event) => setVals({ ...vals, [sp.key]: (e.currentTarget as HTMLInputElement).value });
        let ctl;
        if (sp.kind === "zone") {
          ctl = zs ? (
            <select id={id} value={v} onChange={input}>
              {zones.map((z) => (
                <option key={z} value={z}>
                  {zoneName(z)}
                </option>
              ))}
            </select>
          ) : (
            <input id={id} type="text" value={v} placeholder={bz || "Europe/Berlin"} onInput={input} />
          );
        } else if (sp.kind === "area") {
          ctl = <textarea id={id} rows={7} maxLength={4000} value={v} onInput={input} />;
        } else if (sp.kind === "board") {
          ctl = (
            <select id={id} value={v} onChange={input}>
              <option value="none">None</option>
              <option value="auto">Detect automatically</option>
            </select>
          );
        } else if (sp.kind === "bool") {
          ctl = (
            <select id={id} value={v} disabled={sp.off} onChange={input}>
              <option value="true">On</option>
              <option value="false">Off</option>
              {f.saved ? <option value="">Go back to the default or configuration file</option> : null}
            </select>
          );
        } else if (sp.kind === "time") {
          ctl = <input id={id} type="time" value={v} disabled={sp.off} onInput={input} />;
        } else if (sp.kind === "number") {
          const whole = sp.min !== undefined;
          ctl = (
            <input
              id={id}
              type="number"
              value={v}
              autoComplete="off"
              step={whole ? "1" : "any"}
              min={whole ? String(sp.min) : "0"}
              max={sp.max === undefined ? undefined : String(sp.max)}
              disabled={sp.off}
              onInput={input}
            />
          );
        } else {
          ctl = <input id={id} type="text" value={v} autoComplete="off" disabled={sp.off} onInput={input} />;
        }
        return (
          <div key={sp.key}>
            <label for={id}>{labelOf(sp.key)}</label>
            {ctl}
            <p class="mut">
              {FROM[f.source] ?? f.source}. {f.live ? "Applies right away." : "Needs a restart to apply."}
              {hints[sp.key] ? " " + hints[sp.key] : ""}
              {f.ignored
                ? " A command-line flag is set, so a value saved here has no effect until the flag is removed."
                : ""}
            </p>
          </div>
        );
      })}
      <p class="errtext" role="alert" ref={errRef}>
        {err}
      </p>
      <button>{"Save " + title.toLowerCase()}</button>
    </form>
  );
}

function Details(props: { settings: SettingsData }) {
  const s = props.settings;
  const need = s.restart_needed ?? [];
  const vi = s.voice_info ?? {};
  const rows: [string, string][] = [
    ["Speech language", vi.language || "Automatic"],
    ["Transcription model", vi.stt_model || "None"],
    ["Speech model", vi.tts_model || "None"],
    ["Speakers connect on", s.speakers_listen || "Not listening"],
  ];
  const keyHint =
    "Type the NAME of the environment variable that holds your key, not the key. " +
    (s.llm?.key_present ? "That variable is set." : "That variable is not set where ClawDot runs.");
  return (
    <div>
      <p class="mut">
        Changes are saved on this device. Most apply right away; a few say they need a restart. A command-line flag
        always wins over a value saved here.
      </p>
      {need.length ? (
        <div class="soon" role="status">
          <h3>Restart needed</h3>
          <p>These saved values are not running yet: {restartText(need)}.</p>
          <p>
            ClawDot cannot restart itself safely from this page. Restart it on the device: for example run sudo
            systemctl restart clawdot if it runs as a service, or stop the program and start it again.
          </p>
        </div>
      ) : null}
      <Group
        title="Device"
        settings={s}
        specs={[
          { key: "device_name", kind: "text", hint: "Shown at the top of every page." },
          { key: "zone", kind: "zone", hint: "Reminders and alarms use this time." },
        ]}
      />
      <Group
        title="Language model"
        settings={s}
        intro="Without one, the speaker understands only simple commands like timers. Leave a box empty to go back to the configuration file or the default."
        specs={[
          { key: "llm_url", kind: "text", hint: "Starts with https://, for example https://openrouter.ai/api/v1." },
          { key: "llm_model", kind: "text" },
          { key: "llm_fallback_model", kind: "text", hint: "Tried if the model fails." },
          { key: "summary_model", kind: "text", hint: "A cheaper model for spoken summaries." },
          { key: "llm_key_env", kind: "text", hint: keyHint },
        ]}
      />
      <Group
        title="Personality and voice"
        settings={s}
        specs={[
          {
            key: "persona",
            kind: "area",
            hint: "Instructions for how the assistant talks. Skills are added after this.",
          },
          {
            key: "voice",
            kind: "text",
            hint: s.speech_configured ? "" : "Needs a speech provider in the configuration file.",
            off: !s.speech_configured,
          },
        ]}
      />
      <Group
        title="Addresses"
        settings={s}
        intro="Changing these needs a restart. Use an address on your own network; do not expose this page to the internet."
        specs={[
          { key: "listen", kind: "text", hint: "For example 0.0.0.0:3335 to allow your whole network." },
          { key: "public_url", kind: "text", hint: "Used in the copy-and-paste setup for agents." },
        ]}
      />
      <Group
        title="Usage limits"
        settings={s}
        specs={[
          { key: "max_requests_per_day", kind: "number", hint: "Language model requests per day." },
          { key: "daily_spend_usd", kind: "number", hint: "In US dollars. Empty for no limit." },
        ]}
      />
      <Group
        title="Agent settings"
        settings={s}
        intro="How far the assistant may go to answer one request. Lower numbers make it stop sooner and cost less."
        specs={[
          {
            key: "agent_max_rounds",
            kind: "number",
            min: 1,
            max: 20,
            hint: "How many times it may go back to the language model for one request. 1 to 20.",
          },
          {
            key: "agent_max_tool_calls",
            kind: "number",
            min: 1,
            max: 20,
            hint: "The most tools it may use for one request. 1 to 20.",
          },
          {
            key: "agent_deadline_secs",
            kind: "number",
            min: 5,
            max: 300,
            hint: "After this long it stops and says how far it got. 5 to 300.",
          },
          {
            key: "agent_tool_timeout_secs",
            kind: "number",
            min: 1,
            max: 120,
            hint: "A tool that has not answered by then counts as failed. 1 to 120.",
          },
          {
            key: "plan_speech",
            kind: "bool",
            hint: "Before a request with several steps, it says what it is about to do.",
          },
          {
            key: "progress_speech",
            kind: "bool",
            hint: "Short phrases such as “Checking the lights” while a tool runs.",
          },
          { key: "notes", kind: "bool", hint: "Lets you say “remember that ...”. They are listed in the Notes tab." },
        ]}
      />
      <Group
        title="Daily action limits"
        settings={s}
        intro="A safety net for things the assistant does for you. Reading is never limited."
        specs={[
          {
            key: "max_actions_per_day",
            kind: "number",
            min: 0,
            max: 100000,
            hint: "Anything that changes something, per day. 0 stops all of them.",
          },
          {
            key: "max_costly_per_day",
            kind: "number",
            min: 0,
            max: 100000,
            hint: "The ones that ask you out loud first, which are part of the number above.",
          },
          {
            key: "dry_run",
            kind: "bool",
            hint: "On: it only says what it would do and nothing changes. Good for trying out a new tool.",
          },
        ]}
      />
      <Group
        title="Quiet hours"
        settings={s}
        intro={
          "From the first time to the second, news from your agents waits unless it is urgent. Alarms, timers, and reminders you set still ring. Leave both empty for no quiet hours. Times are in " +
          zoneName(String(sv("zone") || "UTC")) +
          "."
        }
        specs={[
          { key: "quiet_start", kind: "time" },
          { key: "quiet_end", kind: "time", hint: "A window that ends earlier than it starts runs past midnight." },
        ]}
      />
      <Group
        title="Board"
        settings={s}
        specs={[
          {
            key: "board",
            kind: "board",
            hint: "Nothing uses this yet: the hardware layer is not built. The choice is saved for when it is.",
          },
        ]}
      />
      <h3>From the configuration file</h3>
      <dl>
        {rows.map(([k, v]) => (
          <Fragment key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </Fragment>
        ))}
      </dl>
      <p class="mut">These are set in the configuration file only.</p>
    </div>
  );
}

export function Settings(): JSX.Element {
  const s = useApp();
  const [pw, setPw] = useState("");

  const savePassword = async (ev: Event) => {
    ev.preventDefault();
    const password = pw;
    setPw("");
    try {
      const r = await api<{ token: string; recovery_code?: string }>("POST", "/api/admin/password", { password });
      keep(r.token);
      showCode(r.recovery_code);
      say("Password saved.");
      await start();
    } catch (x) {
      fail(x);
    }
  };
  const signOut = async () => {
    try {
      await api("POST", "/api/logout");
    } catch {
      /* signing out always succeeds locally */
    }
    keep("");
    signedOut();
    await start();
  };
  return (
    <>
      <h2>Settings</h2>
      {s.settings ? <Details key={s.settingsRev} settings={s.settings} /> : null}
      <form class="panel" id="pwsec" onSubmit={savePassword}>
        <h3>{s.mode === "open" ? "Set an admin password" : "Change admin password"}</h3>
        {s.legacy ? (
          <p class="mut" id="legacywarn">
            You are signed in with the old administrator token. Set a password to replace it.
          </p>
        ) : null}
        <div class="row">
          <input
            id="newpw"
            type="password"
            autoComplete="new-password"
            placeholder="New password (8 or more characters)"
            aria-label="New password"
            maxLength={128}
            value={pw}
            onInput={(e) => setPw(e.currentTarget.value)}
          />
          <button>Save password</button>
        </div>
      </form>
      <RecoverySettings />
      <About />
      <div class="row">
        <button class="plain" type="button" onClick={signOut}>
          Sign out
        </button>
      </div>
    </>
  );
}
