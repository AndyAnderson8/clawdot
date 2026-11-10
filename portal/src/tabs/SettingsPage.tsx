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
import { RecoverySettings, showCode } from "../recovery.tsx";
import { Skills } from "./Skills.tsx";
import { Tools } from "./Tools.tsx";
import { Webhooks } from "./Webhooks.tsx";

type Spec = SettingSpec[];
const MODEL: Spec = [
  { key: "llm_url", kind: "text", hint: "Starts with https://, for example https://openrouter.ai/api/v1." },
  { key: "llm_model", kind: "text" },
  { key: "llm_fallback_model", kind: "text", hint: "Tried if the model fails." },
  { key: "summary_model", kind: "text", hint: "A cheaper model for spoken summaries." },
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
  { key: "plan_speech", kind: "bool", hint: "Before a request with several steps, it says what it is about to do." },
  { key: "progress_speech", kind: "bool", hint: "Short phrases such as “Checking the lights” while a tool runs." },
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
    <fieldset id="themeform">
      <legend>Theme</legend>
      <div class="choices">
        {options.map(([v, text]) => (
          <label key={v}>
            <input type="radio" name="theme" value={v} checked={pref === v} onChange={() => choose(v)} />
            {text}
          </label>
        ))}
      </div>
      <p class="mut">Light unless you choose otherwise. This is remembered by this browser only.</p>
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
      <h3>Language</h3>
      <dl class="kv">
        <dt>Speech language</dt>
        <dd>{vi.language || "Automatic"}</dd>
      </dl>
      <p class="mut">The speech language is set in the configuration file.</p>
      <h3>Appearance</h3>
      <ThemeChoice />
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
      <Group
        title="How far it may go"
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
      <div id="part-tools">
        <Tools />
      </div>
      <div id="part-connections">
        <Connections />
      </div>
      <div id="part-skills">
        <Skills />
      </div>
    </>
  );
}

function NotesSection(props: { s: SettingsData }): JSX.Element {
  return (
    <>
      <Notes />
      <Group
        title="Keeping notes"
        settings={props.s}
        specs={[{ key: "notes", kind: "bool", hint: "Lets you say “remember that ...”. They are listed above." }]}
      />
    </>
  );
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
      <form class="panel" id="pwsec" onSubmit={savePassword}>
        <h3>{s.mode === "open" ? "Set an admin password" : "Change admin password"}</h3>
        {s.legacy ? (
          <p class="mut" id="legacywarn">
            You are signed in with the old administrator token. Set a password to replace it.
          </p>
        ) : null}
        <label for="newpw">New password</label>
        <div class="row">
          <input
            id="newpw"
            type="password"
            autoComplete="new-password"
            placeholder="8 or more characters"
            maxLength={128}
            value={pw}
            onInput={(e) => setPw(e.currentTarget.value)}
          />
          <button>Save password</button>
        </div>
      </form>
      <RecoverySettings />
      <h3>This browser</h3>
      <div class="row">
        <button class="plain" type="button" onClick={signOut}>
          Sign out
        </button>
      </div>
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
      <dl class="kv">
        {rows.map(([k, v]) => (
          <Fragment key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </Fragment>
        ))}
      </dl>
      <p class="mut">These are set in the configuration file only.</p>
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
  { id: "assistant", label: "Assistant", view: Assistant },
  { id: "inputs", label: "Inputs", view: Inputs },
  { id: "tools", label: "Tools and connections", view: Extensions },
  { id: "notes", label: "Notes", view: NotesSection },
  { id: "safety", label: "Safety", view: Safety },
  { id: "security", label: "Security", view: Security },
  { id: "about", label: "About and diagnostics", view: AboutSection },
];

/** Everything that can be configured, in sections. A phone shows the list, then one section at a time. */
export function SettingsPage(props: { sub: string }): JSX.Element {
  const s = useApp();
  const known = SECTIONS.find((x) => x.id === props.sub);
  const active = known ?? SECTIONS[0];
  const need = s.settings?.restart_needed ?? [];
  const View = active.view;
  const sub = known ? "yes" : "no";
  return (
    <>
      <h2 class="stitle" data-sub={sub}>
        Settings
      </h2>
      <div class="settings" data-sub={sub}>
        <nav aria-label="Settings sections" class="sections">
          <ul class="index">
            {SECTIONS.map((x) => (
              <li key={x.id}>
                <a href={"#/settings/" + x.id} aria-current={x.id === active.id ? "page" : undefined}>
                  {x.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <section class="pane" id="pane" aria-labelledby="sechead">
          <a class="back" href="#/settings">
            &lsaquo; Settings
          </a>
          <h3 class="sechead" id="sechead">
            {active.label}
          </h3>
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
    </>
  );
}
