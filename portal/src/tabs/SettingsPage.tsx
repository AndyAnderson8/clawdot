import type { JSX } from "preact";
import { Fragment } from "preact";
import { useState } from "preact/hooks";
import { api, keep } from "../api.ts";
import { restartText, type SettingSpec } from "../forms.ts";
import { fail, say, signedOut, start, sv, useApp } from "../store.ts";
import { setThemePref, themePref, type ThemePref } from "../theme.ts";
import { zoneName } from "../time.ts";
import type { Settings as SettingsData } from "../types.ts";
import { About } from "./About.tsx";
import { Connections } from "./Connections.tsx";
import { Notes } from "./Notes.tsx";
import { Group } from "./Settings.tsx";
import { PROVIDERS, providerFor } from "../providers.ts";
import { Field } from "../parts.tsx";
import { FeaturesSection } from "./Features.tsx";
import { Skills } from "./Skills.tsx";
import { TestSpeaker } from "./TestSpeaker.tsx";
import { Tools } from "./Tools.tsx";
import { Webhooks } from "./Webhooks.tsx";

type Spec = SettingSpec[];
const MODEL: Spec = [
  {
    key: "llm_url",
    kind: "text",
    hint: "Starts with https:// (or http:// for a model on your own network).",
    hideWhen: (v) => {
      const p = providerFor(v["llm_url"] ?? "");
      return !p || (p.id !== "custom" && p.id !== "local");
    },
  },
  { key: "llm_model", kind: "text" },
  { key: "llm_fallback_model", kind: "text", hint: "Tried if the model fails.", optional: "Add a backup model" },
  {
    key: "summary_model",
    kind: "text",
    hint: "A cheaper model for spoken summaries.",
    optional: "Add a model for summaries",
  },
];
const LIMITS: Spec = [
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
];

function ThemeChoice(): JSX.Element {
  const [pref, setPref] = useState<ThemePref>(themePref());
  const choose = (p: ThemePref) => {
    setPref(p);
    setThemePref(p);
  };
  const options: [ThemePref, string][] = [
    ["light", "Light"],
    ["dark", "Dark"],
    ["system", "Match system"],
  ];
  return (
    <fieldset id="themeform" class="srow">
      <div class="slabel">
        <legend>Theme</legend>
        <p class="mut">Light unless you choose otherwise. This is remembered by this browser only.</p>
      </div>
      <div class="choices sctl">
        {options.map(([v, text]) => (
          <label key={v}>
            <input type="radio" name="theme" value={v} checked={pref === v} onChange={() => choose(v)} />
            {text}
          </label>
        ))}
      </div>
    </fieldset>
  );
}

function General(props: { s: SettingsData }): JSX.Element {
  const vi = props.s.voice_info ?? {};
  return (
    <>
      <Group
        title="Device"
        settings={props.s}
        specs={[
          { key: "device_name", kind: "text", hint: "Shown at the top of every page." },
          { key: "zone", kind: "zone", hint: "Reminders and alarms use this time." },
        ]}
      />
      <section class="card">
        <h3>Language</h3>
        <dl class="kv">
          <dt>Speech language</dt>
          <dd>{vi.language || "Automatic"}</dd>
        </dl>
        <p class="mut">The speech language is set in the configuration file.</p>
      </section>
      <section class="card sgroup">
        <h3>Appearance</h3>
        <ThemeChoice />
      </section>
    </>
  );
}

function Assistant(props: { s: SettingsData }): JSX.Element {
  const s = props.s;
  const keyHint =
    "Type the NAME of the environment variable that holds your key, not the key. " +
    (s.llm?.key_present ? "That variable is set." : "That variable is not set where ClawDot runs.");
  return (
    <>
      <Group
        title="Language model"
        settings={s}
        extra={({ vals, setVals }) => {
          const cur = providerFor(vals["llm_url"] ?? "");
          return (
            <div class="srow">
              <div class="slabel">
                <label for="provider">Who answers</label>
                <p class="mut">{cur?.note ?? "Pick a service. You can change it at any time."}</p>
              </div>
              <div class="sctl">
                <select
                  id="provider"
                  required
                  value={cur?.id ?? ""}
                  onChange={(e) => {
                    const p = PROVIDERS.find((x) => x.id === e.currentTarget.value);
                    if (!p) return;
                    setVals({
                      ...vals,
                      llm_url: p.id === "custom" ? "https://" : p.url,
                      llm_model: p.model || vals["llm_model"] || "",
                      llm_key_env: p.keyEnv || vals["llm_key_env"] || "",
                    });
                  }}
                >
                  {cur ? null : (
                    <option value="" disabled>
                      Choose a service
                    </option>
                  )}
                  {PROVIDERS.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </select>
              </div>
            </div>
          );
        }}
        intro="Without one, the speaker understands only simple commands like timers. Leave a box empty to go back to the configuration file or the default."
        specs={[...MODEL, { key: "llm_key_env", kind: "text", hint: keyHint }]}
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
      <TestSpeaker />
      <Group
        title="Limits for one request"
        settings={s}
        intro="How far the assistant may go to answer one request. Lower numbers make it stop sooner and cost less."
        specs={LIMITS}
      />
    </>
  );
}

function Inputs(props: { s: SettingsData }): JSX.Element {
  return (
    <>
      <Webhooks />
      <Group
        title="Addresses"
        settings={props.s}
        intro="Changing these needs a restart. Use an address on your own network; do not expose this page to the internet."
        specs={[
          { key: "listen", kind: "text", hint: "For example 0.0.0.0:3335 to allow your whole network." },
          { key: "public_url", kind: "text", hint: "Used in the copy-and-paste setup for inputs." },
        ]}
      />
    </>
  );
}

function Extensions(): JSX.Element {
  return (
    <>
      <div id="part-tools" class="stack">
        <Tools />
      </div>
      <div id="part-connections" class="stack">
        <Connections />
      </div>
      <div id="part-skills" class="stack">
        <Skills />
      </div>
    </>
  );
}

function MemorySection(): JSX.Element {
  return <Notes />;
}

function Safety(props: { s: SettingsData }): JSX.Element {
  const s = props.s;
  return (
    <>
      <Group
        title="Usage limits"
        settings={s}
        specs={[
          { key: "max_requests_per_day", kind: "number", hint: "Language model requests per day." },
          { key: "daily_spend_usd", kind: "number", hint: "In US dollars. Empty for no limit." },
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
        ]}
      />
      <Group
        title="Quiet hours"
        settings={s}
        intro={
          "From the first time to the second, events from your inputs wait unless they are urgent. Alarms, timers, and reminders you set still ring. Leave both empty for no quiet hours. Times are in " +
          zoneName(String(sv("zone") || "UTC")) +
          "."
        }
        specs={[
          { key: "quiet_start", kind: "time" },
          { key: "quiet_end", kind: "time", hint: "A window that ends earlier than it starts runs past midnight." },
        ]}
      />
    </>
  );
}

function Security(): JSX.Element {
  const s = useApp();
  const [pw, setPw] = useState("");
  const savePassword = async (ev: Event) => {
    ev.preventDefault();
    const password = pw;
    setPw("");
    try {
      const r = await api<{ token: string }>("POST", "/api/admin/password", { password });
      keep(r.token);
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
      <form class="panel" id="pwsec" onSubmit={savePassword}>
        <h3>{s.mode === "open" ? "Set an admin password" : "Change admin password"}</h3>
        {s.legacy ? (
          <p class="mut" id="legacywarn">
            You are signed in with the old administrator token. Set a password to replace it.
          </p>
        ) : null}
        <Field id="newpw" label="New password" hint="At least 8 characters.">
          <input
            id="newpw"
            type="password"
            autoComplete="new-password"
            placeholder="New password"
            maxLength={128}
            value={pw}
            onInput={(e) => setPw(e.currentTarget.value)}
          />
        </Field>
        <div class="row">
          <button>Save password</button>
        </div>
      </form>
      <section class="card sgroup">
        <h3>This browser</h3>
        <div class="srow">
          <div class="slabel">
            <label>Sign out</label>
            <p class="mut">You will need the password to come back.</p>
          </div>
          <div class="sctl">
            <button class="plain" type="button" onClick={signOut}>
              Sign out
            </button>
          </div>
        </div>
      </section>
    </>
  );
}

function AboutSection(props: { s: SettingsData }): JSX.Element {
  const s = props.s;
  const vi = s.voice_info ?? {};
  const rows: [string, string][] = [
    ["Speech language", vi.language || "Automatic"],
    ["Transcription model", vi.stt_model || "None"],
    ["Speech model", vi.tts_model || "None"],
    ["Speakers connect on", s.speakers_listen || "Not listening"],
  ];
  return (
    <>
      <About />
      <Group
        title="Hardware"
        settings={s}
        specs={[
          {
            key: "board",
            kind: "board",
            hint: "Nothing uses this yet: the hardware layer is not built. The choice is saved for when it is.",
          },
        ]}
      />
      <section class="card">
        <h3>From the configuration file</h3>
        <p class="mut">These are set in the configuration file only.</p>
        <dl class="kv">
          {rows.map(([k, v]) => (
            <Fragment key={k}>
              <dt>{k}</dt>
              <dd>{v}</dd>
            </Fragment>
          ))}
        </dl>
      </section>
    </>
  );
}

interface Section {
  id: string;
  label: string;
  view: (p: { s: SettingsData }) => JSX.Element;
}
export const SECTIONS: [Section, ...Section[]] = [
  { id: "general", label: "General", view: General },
  { id: "features", label: "Features", view: FeaturesSection },
  { id: "assistant", label: "Assistant", view: Assistant },
  { id: "inputs", label: "Inputs", view: Inputs },
  { id: "tools", label: "Tools", view: Extensions },
  { id: "memory", label: "Memory", view: MemorySection },
  { id: "safety", label: "Safety", view: Safety },
  { id: "security", label: "Security", view: Security },
  { id: "about", label: "About", view: AboutSection },
];

/** Everything you set once, in sections along the top. */
export function SettingsPage(props: { sub: string }): JSX.Element {
  const s = useApp();
  const active = SECTIONS.find((x) => x.id === props.sub) ?? SECTIONS[0];
  const need = s.settings?.restart_needed ?? [];
  const View = active.view;
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Settings</h2>
        <p>Everything you can change, in sections. Features are the switches for what the assistant does.</p>
      </header>
      <nav aria-label="Settings sections" class="tabs">
        {SECTIONS.map((x) => (
          <a key={x.id} href={"#/settings/" + x.id} aria-current={x.id === active.id ? "page" : undefined}>
            {x.label}
          </a>
        ))}
      </nav>
      <section class="pane stack" id="pane" aria-label={active.label}>
        {need.length ? (
          <div class="soon" role="status">
            <h4>Restart needed</h4>
            <p>These saved values are not running yet: {restartText(need)}.</p>
            <p>
              ClawDot cannot restart itself safely from this page. Restart it on the device: for example run sudo
              systemctl restart clawdot if it runs as a service, or stop the program and start it again.
            </p>
          </div>
        ) : null}
        {s.settings ? <View key={s.settingsRev} s={s.settings} /> : <p class="empty">Loading the settings.</p>}
      </section>
    </div>
  );
}
