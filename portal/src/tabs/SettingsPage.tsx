import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, keep } from "../api.ts";
import { restartText, type SettingSpec } from "../forms.ts";
import { fail, say, signedOut, start, sv, useApp } from "../store.ts";
import { setThemePref, themePref, type ThemePref } from "../theme.ts";
import { zoneName } from "../time.ts";
import type { Settings as SettingsData } from "../types.ts";
import { About } from "./About.tsx";
import { Sources } from "./Connections.tsx";
import { Notes } from "./Notes.tsx";
import { Group } from "./Settings.tsx";
import { PROVIDERS, providerFor } from "../providers.ts";
import { Field } from "../parts.tsx";
import { TestSpeaker } from "./TestSpeaker.tsx";
import { FeaturesSection } from "./Features.tsx";
import { Skills } from "./Skills.tsx";
import { Lights } from "./Lights.tsx";
import { MediaSettings } from "./Media.tsx";
import { Tools } from "./Tools.tsx";
import { Webhooks } from "./Webhooks.tsx";

type Spec = SettingSpec[];
const MODEL: Spec = [
  {
    key: "llm_url",
    kind: "text",
    hint: "Where the service is. It starts with https://, or http:// for a model on your own network.",
    hideWhen: (v) => {
      const p = providerFor(v["llm_url"] ?? "");
      return !p || (p.id !== "custom" && p.id !== "local");
    },
  },
  { key: "llm_model", kind: "model", hint: "The model that answers you. The list comes from your provider." },
  {
    key: "llm_fallback_model",
    kind: "model",
    hint: "Used instead if the main model is unavailable.",
    optional: "Add a backup model",
  },
  {
    key: "summary_model",
    kind: "model",
    hint: "A cheaper model for reading long results aloud in a few words.",
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
    "The name of the environment variable that holds your key, for example OPENROUTER_API_KEY. ClawDot never stores the key itself. " +
    (s.llm?.key_present ? "That variable is set." : "That variable is not set where ClawDot runs.");
  return (
    <>
      <Group
        title="Provider and model"
        settings={s}
        intro="The service that runs the AI behind your speaker. Without one, the speaker understands only simple commands such as timers."
        specs={[...MODEL.slice(0, 1), { key: "llm_key_env", kind: "text", hint: keyHint }, ...MODEL.slice(1)]}
        extra={({ vals, setVals }) => {
          const cur = providerFor(vals["llm_url"] ?? "");
          return (
            <div class="srow">
              <div class="slabel">
                <label for="provider">Provider</label>
                <p class="mut">{cur?.note ?? "Choose the service that will answer you."}</p>
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
                      llm_model: p.id === "custom" ? "" : p.model,
                      llm_key_env: p.keyEnv || vals["llm_key_env"] || "",
                    });
                  }}
                >
                  {cur ? null : (
                    <option value="" disabled>
                      Choose a provider
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
      />
      <Group
        title="Personality"
        settings={s}
        intro="How the assistant talks. The skills you turn on are added after this."
        specs={[
          {
            key: "persona",
            kind: "area",
            hint: "Written to the assistant, in your own words.",
          },
        ]}
      />
      <Group
        title="Limits for one request"
        settings={s}
        intro="How far the assistant may go to answer one request. Lower numbers make it stop sooner and cost less."
        specs={LIMITS}
      />
      <Group
        title="Spending limit"
        settings={s}
        intro="Stops the assistant from using the language model once it has cost this much in a day. It starts again the next day."
        specs={[{ key: "daily_spend_usd", kind: "number", hint: "In US dollars. Leave it empty for no limit." }]}
      />
      <Group
        title="Quiet hours"
        settings={s}
        intro={
          "From the first time to the second, messages from your senders wait unless they are urgent. Alarms, timers, and reminders you set still ring. Leave both empty for no quiet hours. Times are in " +
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

/** Speech: the provider (shown, not yet changeable here), the voice and language, and a way to hear it. */
function VoiceSection(props: { s: SettingsData }): JSX.Element {
  const s = props.s;
  const vi = s.voice_info ?? {};
  return (
    <>
      <section class="card" id="speechprovider">
        <h3>Speech provider</h3>
        <p class="mut">
          The service that turns what you say into text and the assistant&apos;s words into speech. It is chosen when
          ClawDot is installed. Changing it from this page is not available yet.
        </p>
        {s.speech_configured ? (
          <dl class="kv">
            <dt>Provider</dt>
            <dd>{vi.provider || "Not known"}</dd>
            <dt>Hears you with</dt>
            <dd>{vi.stt_model || "Not set"}</dd>
            <dt>Speaks with</dt>
            <dd>{vi.tts_model || "Not set"}</dd>
          </dl>
        ) : (
          <p class="empty">
            No speech provider is set up, so the speaker cannot hear you or talk yet. Whoever installed ClawDot can add
            one; the local speech guide shows a free one.
          </p>
        )}
      </section>
      <Group
        title="Voice and language"
        settings={s}
        specs={[
          {
            key: "voice",
            kind: "voice",
            hint: s.speech_configured ? "How the assistant sounds." : "Needs a speech provider.",
            off: !s.speech_configured,
          },
          {
            key: "speech_language",
            kind: "language",
            hint: s.speech_configured
              ? "The language you speak to it. Detecting it works for most people; naming it is more accurate."
              : "Needs a speech provider.",
            off: !s.speech_configured,
          },
        ]}
      />
      <TestSpeaker />
    </>
  );
}

function Notifications(props: { s: SettingsData }): JSX.Element {
  return (
    <>
      <Webhooks />
      <Group
        title="Where ClawDot can be reached"
        settings={props.s}
        intro="Changing these needs a restart. Use an address on your own network; do not expose this page to the internet."
        specs={[
          {
            key: "listen",
            kind: "text",
            hint: "Where ClawDot serves this page and listens for senders. For example 0.0.0.0:3335 lets your whole network reach it.",
          },
          { key: "public_url", kind: "text", hint: "The address a sender posts to. It appears in the setup snippets." },
        ]}
      />
    </>
  );
}

function ToolsSection(): JSX.Element {
  return (
    <>
      <div id="part-sources" class="stack">
        <Sources />
      </div>
      <div id="part-tools" class="stack">
        <Tools />
      </div>
    </>
  );
}

function SkillsSection(): JSX.Element {
  return (
    <div id="part-skills" class="stack">
      <Skills />
    </div>
  );
}

function MemorySection(): JSX.Element {
  return <Notes />;
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
  { id: "voice", label: "Voice", view: VoiceSection },
  { id: "tools", label: "Tools", view: ToolsSection },
  { id: "skills", label: "Skills", view: SkillsSection },
  { id: "media", label: "Media", view: MediaSettings },
  { id: "lights", label: "Lights", view: Lights },
  { id: "memory", label: "Memory", view: MemorySection },
  { id: "notifications", label: "Notifications", view: Notifications },
  { id: "security", label: "Security", view: Security },
  { id: "about", label: "About", view: AboutSection },
];

/** Everything you set once, in sections along the top. */
export function SettingsPage(props: { sub: string }): JSX.Element {
  const s = useApp();
  const sub = props.sub === "inputs" ? "notifications" : props.sub === "safety" ? "assistant" : props.sub;
  const active = SECTIONS.find((x) => x.id === sub) ?? SECTIONS[0];
  const need = s.settings?.restart_needed ?? [];
  const View = active.view;
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Settings</h2>
        <p>Everything you can change, in sections. Features are the switches for what the assistant does.</p>
      </header>
      <div class="settings">
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
    </div>
  );
}
