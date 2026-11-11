import { Fragment, type JSX } from "preact";
import { useState } from "preact/hooks";
import { api, keep } from "../api.ts";
import { restartText, type SettingSpec } from "../forms.ts";
import { fail, say, signedOut, start, sv, useApp } from "../store.ts";
import { zoneName } from "../time.ts";
import type { HardwareInfo, Settings as SettingsData } from "../types.ts";
import { About } from "./About.tsx";
import { Sources } from "./Connections.tsx";
import { Notes } from "./Notes.tsx";
import { Group } from "./Settings.tsx";
import { Field } from "../parts.tsx";
import { PROVIDERS, providerFor } from "../providers.ts";
import { TestSpeaker } from "./TestSpeaker.tsx";
import { FeaturesSection } from "./Features.tsx";
import { Skills } from "./Skills.tsx";
import { HardwareCard } from "./HardwareCard.tsx";
import { Lights } from "./Lights.tsx";
import { Planned } from "./Planned.tsx";
import { SavedKey } from "./SavedKey.tsx";
import { Speakers } from "./Speakers.tsx";
import { Themes } from "./Themes.tsx";
import { MediaSettings } from "./Media.tsx";
import { Tools } from "./Tools.tsx";
import { Webhooks } from "./Webhooks.tsx";
import { ask } from "../confirm.tsx";

type Spec = SettingSpec[];
const MODEL: Spec = [
  {
    key: "llm_url",
    kind: "text",
    hint: "Where the service is. It starts with https://, or http:// for a model you run yourself.",
    hideWhen: (v: Record<string, string>) => providerFor(v["llm_url"] ?? "")?.id !== "custom",
  },
  {
    key: "llm_key_env",
    kind: "text",
    hint: "Optional. Instead of pasting a key, name an environment variable on the device that holds it.",
    hideWhen: (v: Record<string, string>) => providerFor(v["llm_url"] ?? "")?.id !== "custom",
  },
  { key: "llm_model", kind: "model", hint: "The model that answers you. The list comes from your provider." },
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

function ThemesSection(): JSX.Element {
  return <Themes />;
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
      <HardwareCard />
      <Group
        title="Hardware support"
        settings={props.s}
        intro="How ClawDot reaches this device's own parts, such as its lights."
        specs={[
          {
            key: "board",
            kind: "board",
            hint: "Not used yet: the hardware layer is not built. The choice is saved for when it is. Choose None for a device ClawDot does not know.",
          },
        ]}
      />
    </>
  );
}

/** The provider's key: pasted once, checked with the service, kept on the device, and shown only as its first and last few characters. */
function KeyRow(props: {
  s: SettingsData;
  vals: Record<string, string>;
  setVals: (v: Record<string, string>) => void;
}): JSX.Element | null {
  const { s, vals, setVals } = props;
  const prov = providerFor(vals["llm_url"] ?? "");
  const [typed, setTyped] = useState("");
  const [replacing, setReplacing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const [bad, setBad] = useState(false);
  if (!prov) return null;
  const hint = vals["_keyhint"] ?? s.llm?.key_hint ?? "";
  const fromEnv = !hint && s.llm?.key_present === true && vals["_keyok"] !== "0";
  const check = async () => {
    const key = typed.trim();
    if (!key) {
      setMsg("Paste the key first.");
      setBad(true);
      return;
    }
    setBusy(true);
    setMsg("");
    try {
      // The service is asked first, so a key that does not work is never kept.
      const r = await api<{ models: string[] }>("POST", "/api/models", { url: vals["llm_url"] ?? "", key });
      const k = await api<{ hint: string | null }>("PUT", "/api/llm/key", { key });
      setTyped("");
      setReplacing(false);
      setBad(false);
      setMsg(
        "Key accepted. " + r.models.length + (r.models.length === 1 ? " model is" : " models are") + " available.",
      );
      setVals({ ...vals, _keyhint: k.hint ?? "", _keyok: "1", _keyrev: String(Date.now()) });
    } catch (x) {
      setBad(true);
      setMsg(x instanceof Error ? x.message : "The key could not be checked.");
    }
    setBusy(false);
  };
  const remove = async () => {
    if (!(await ask("Remove the saved key? The assistant will stop answering until you add one.", "Remove"))) return;
    try {
      await api("DELETE", "/api/llm/key");
      setMsg("");
      setVals({ ...vals, _keyhint: "", _keyok: "0", _keyrev: String(Date.now()) });
    } catch (x) {
      fail(x);
    }
  };
  const showing = hint && !replacing;
  return (
    <div class="srow" id="keyrow">
      <div class="slabel">
        <label for="apikey">API key</label>
        <p class="mut">
          {prov.id === "custom"
            ? "If the service needs one. It is checked, then kept on this device; only its first and last few characters are shown."
            : "From your " +
              prov.name +
              " account. It is checked, then kept on this device; only its first and last few characters are shown."}
        </p>
      </div>
      <div class="sctl">
        {showing ? (
          <div class="row keyshown">
            <code id="keyhint" class="keyhint">
              {hint}
            </code>
            <button class="plain small" type="button" id="keyreplace" onClick={() => setReplacing(true)}>
              Replace
            </button>
            <button class="plain small" type="button" id="keyremove" onClick={() => void remove()}>
              Remove
            </button>
          </div>
        ) : (
          <div class="keyentry">
            {fromEnv ? <p class="mut">Using the key from the environment variable on the device.</p> : null}
            <div class="row">
              <input
                id="apikey"
                type="password"
                autoComplete="off"
                placeholder="Paste your API key"
                value={typed}
                onInput={(e) => setTyped(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void check();
                  }
                }}
              />
              <button class="plain small" type="button" id="keycheck" disabled={busy} onClick={() => void check()}>
                {busy ? "Checking..." : "Check key"}
              </button>
              {replacing ? (
                <button class="plain small" type="button" onClick={() => setReplacing(false)}>
                  Cancel
                </button>
              ) : null}
            </div>
          </div>
        )}
        {msg ? (
          <p class={bad ? "errtext" : "mut"} role={bad ? "alert" : "status"} id="keymsg">
            {msg}
          </p>
        ) : null}
      </div>
    </div>
  );
}

function Assistant(props: { s: SettingsData }): JSX.Element {
  const s = props.s;
  return (
    <>
      <Group
        title="Provider and model"
        settings={s}
        intro="The service that runs the AI behind your speaker. Without one, the speaker understands only simple commands such as timers."
        specs={MODEL}
        extra={({ vals, setVals }) => {
          const cur = providerFor(vals["llm_url"] ?? "");
          return (
            <>
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
                        _keyrev: String(Date.now()),
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
              <KeyRow s={s} vals={vals} setVals={setVals} />
            </>
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
        specs={[{ key: "daily_spend_usd", kind: "money", hint: "In US dollars. Leave it empty for no limit." }]}
      />
      <Group
        title="Quiet hours"
        settings={s}
        intro={
          "From the first time to the second, messages from your webhooks wait unless they are urgent. Alarms, timers, and reminders you set still ring. Leave both empty for no quiet hours. Times are in " +
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
const SPEECH_SERVICES: [string, string][] = [
  ["openai", "An OpenAI-style speech server"],
  ["elevenlabs", "ElevenLabs"],
  ["deepgram", "Deepgram"],
];

function VoiceSection(props: { s: SettingsData }): JSX.Element {
  const s = props.s;
  const vi = s.voice_info ?? {};
  const live = useApp().status?.realtime;
  const service = String(s.fields["speech_provider"]?.value ?? "openai");
  const own = service !== "openai";
  const rt = s.realtime;
  const noRealtime = rt?.built === false;
  return (
    <>
      <Group
        title="Speech provider"
        settings={s}
        intro="The service that turns what you say into text and the assistant's words into speech. Changing it needs a restart, and a new voice, because voices do not carry from one service to another."
        specs={[
          {
            key: "speech_provider",
            kind: "choice",
            options: SPEECH_SERVICES,
            hint: "ElevenLabs and Deepgram need only a key. An OpenAI-style server is set up where ClawDot is installed.",
          },
        ]}
        below={() => (
          <>
            <SavedKey
              id="speech"
              label="Key"
              what={
                own
                  ? "From your account with the service. It is kept on this device; only its first and last few characters are shown."
                  : "Only if your speech server asks for one. It is kept on this device; only its first and last few characters are shown."
              }
              path="/api/speech/key"
              hint={s.speech?.key_hint}
              fromEnv={s.speech?.key_present === true}
            />
            <div id="speechprovider">
              {s.speech_configured || own ? (
                <dl class="kv">
                  <dt>In use now</dt>
                  <dd>{vi.provider || "Not known"}</dd>
                  <dt>Hears you with</dt>
                  <dd>{vi.stt_model || "The service's usual model"}</dd>
                  <dt>Speaks with</dt>
                  <dd>{vi.tts_model || "The service's usual model"}</dd>
                </dl>
              ) : (
                <p class="empty">
                  No speech provider is set up, so the speaker cannot hear you or talk yet. Choose ElevenLabs or
                  Deepgram above and add a key, or follow the local speech guide for a free one.
                </p>
              )}
            </div>
          </>
        )}
      />
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
      <Group
        title="Realtime voice"
        settings={s}
        intro="An optional service that hears you and answers in one step, which is faster than listening, thinking and speaking in turn. It is billed by the minute of audio, and your spending limit counts it. Tools still ask you first. Off unless you turn it on."
        specs={[
          { key: "realtime_enabled", kind: "bool", off: noRealtime },
          {
            key: "realtime_url",
            kind: "text",
            off: noRealtime,
            hint: "It starts with wss://. For example wss://api.openai.com/v1/realtime.",
          },
          { key: "realtime_model", kind: "text", off: noRealtime, hint: "The realtime model's name." },
          { key: "realtime_voice", kind: "text", off: noRealtime, hint: "A voice name the service offers." },
          {
            key: "realtime_usd_per_min_in",
            kind: "money",
            off: noRealtime,
            placeholder: "0.03",
            hint: "What the service charges for a minute of your speech. Used to count spending; check it against your bill.",
          },
          {
            key: "realtime_usd_per_min_out",
            kind: "money",
            off: noRealtime,
            placeholder: "0.10",
            hint: "What it charges for a minute of its speech.",
          },
          {
            key: "realtime_max_session_secs",
            kind: "number",
            min: 10,
            max: 3600,
            off: noRealtime,
            hint: "In seconds. A conversation ends by itself after this long. 10 to 3600.",
          },
        ]}
        below={() => (
          <>
            <SavedKey
              id="realtime"
              label="Key"
              what="From your account with the realtime service. It is kept on this device; only its first and last few characters are shown."
              path="/api/realtime/key"
              hint={rt?.key_hint}
              off={noRealtime ? "Not available in this build" : undefined}
            />
            <p class="mut" id="realtimestate" role="status">
              {noRealtime
                ? "This build of ClawDot does not include realtime voice."
                : live?.active
                  ? "A realtime conversation is going on now."
                  : live?.last_error
                    ? "The last realtime conversation failed: " + live.last_error
                    : rt?.configured
                      ? "Ready. The next conversation after the wake word uses it."
                      : "Not ready: it needs to be turned on, with an address, a model and a key."}
            </p>
          </>
        )}
      />
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
            hint: "Where ClawDot serves this page and listens for webhooks. For example 0.0.0.0:3335 lets your whole network reach it.",
          },
          {
            key: "public_url",
            kind: "text",
            hint: "The address other programs post webhook messages to. It appears in the setup snippets.",
          },
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

function AboutSection(): JSX.Element {
  return (
    <>
      <About />
    </>
  );
}

interface Section {
  id: string;
  label: string;
  group: string;
  view: (p: { s: SettingsData }) => JSX.Element;
  /** The device must have this for the section to be useful; otherwise it is listed only when asked for. */
  needs?: (hw: HardwareInfo) => boolean;
  /** What to say when the device lacks what it needs. */
  why?: string;
  /** Not built yet: shown (as a plan) only when asked for. */
  planned?: boolean;
}
const GROUPS = ["Basics", "Assistant", "Integrations", "System"] as const;
export const SECTIONS: [Section, ...Section[]] = [
  { id: "general", label: "General", group: "Basics", view: General },
  { id: "themes", label: "Themes", group: "Basics", view: ThemesSection },
  { id: "features", label: "Features", group: "Basics", view: FeaturesSection },
  { id: "assistant", label: "Assistant", group: "Assistant", view: Assistant },
  { id: "voice", label: "Voice", group: "Assistant", view: VoiceSection },
  { id: "tools", label: "Tools", group: "Assistant", view: ToolsSection },
  { id: "skills", label: "Skills", group: "Assistant", view: SkillsSection },
  { id: "memory", label: "Memory", group: "Assistant", view: MemorySection },
  { id: "media", label: "Media", group: "Integrations", view: MediaSettings },
  {
    id: "lights",
    label: "Lights",
    group: "Integrations",
    view: Lights,
    needs: (hw) => hw.parts.leds,
    why: "No light ring was found on this device, so these settings would do nothing here.",
  },
  { id: "webhooks", label: "Webhooks", group: "Integrations", view: Notifications },
  { id: "speakers", label: "Speakers", group: "Integrations", view: () => <Speakers /> },
  {
    id: "homeassistant",
    label: "Home Assistant",
    group: "Integrations",
    view: () => (
      <Planned
        title="Home Assistant"
        text="Use your Home Assistant devices and automations from the assistant. Until this is built in, add Home Assistant as a tool source on the Tools page."
      />
    ),
    planned: true,
  },
  {
    id: "zigbee",
    label: "Zigbee",
    group: "Integrations",
    view: () => (
      <Planned
        title="Zigbee"
        text="Pair and control Zigbee lights, plugs and sensors directly, with no hub. It needs a Zigbee adapter, such as a USB stick."
      />
    ),
    needs: (hw) => hw.parts.usb_serial,
    why: "No USB serial adapter was found, so there is no Zigbee adapter to use.",
    planned: true,
  },
  {
    id: "intercom",
    label: "Intercom",
    group: "Integrations",
    view: () => <Planned title="Intercom" text="Talk between the speakers in your home." />,
    planned: true,
  },
  { id: "security", label: "Security", group: "System", view: Security },
  { id: "about", label: "About", group: "System", view: AboutSection },
];

/** The sections to list: those the device can use, and the rest only when asked for. */
export function visibleSections(hw: HardwareInfo | undefined, showAll: boolean): Section[] {
  return SECTIONS.filter((x) => showAll || (!x.planned && (!hw || !x.needs || x.needs(hw))));
}

const SHOW_ALL = "clawdot-show-unavailable";
function showAllPref(): boolean {
  try {
    return localStorage.getItem(SHOW_ALL) === "1";
  } catch {
    return false;
  }
}

/** Everything you set once: a list of sections down the side, in groups, and the chosen one beside it. */
export function SettingsPage(props: { sub: string }): JSX.Element {
  const s = useApp();
  const [showAll, setShowAll] = useState(showAllPref());
  const alias: Record<string, string> = { inputs: "webhooks", notifications: "webhooks", safety: "assistant" };
  const sub = alias[props.sub] ?? props.sub;
  const active = SECTIONS.find((x) => x.id === sub) ?? SECTIONS[0];
  const need = s.settings?.restart_needed ?? [];
  const View = active.view;
  const hw = s.status?.hardware;
  // The open section is always in the list, even if the device cannot use it.
  const shown = new Set(visibleSections(hw, showAll));
  shown.add(active);
  const list = SECTIONS.filter((x) => shown.has(x));
  const lacking = hw && active.needs && !active.needs(hw) ? (active.why ?? "This device lacks a part this needs.") : "";
  const toggle = (on: boolean) => {
    setShowAll(on);
    try {
      localStorage.setItem(SHOW_ALL, on ? "1" : "0");
    } catch {
      /* the choice lasts until the page is closed */
    }
  };
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Settings</h2>
        <p>Everything you can change, in sections. Features are the switches for what the assistant does.</p>
      </header>
      <div class="settings">
        <nav aria-label="Settings sections" class="tabs">
          {GROUPS.map((g) => {
            const items = list.filter((x) => x.group === g);
            return items.length ? (
              <Fragment key={g}>
                <span class="navhead">{g}</span>
                {items.map((x) => (
                  <a key={x.id} href={"#/settings/" + x.id} aria-current={x.id === active.id ? "page" : undefined}>
                    {x.label}
                  </a>
                ))}
              </Fragment>
            ) : null;
          })}
          <label class="showall">
            <input id="showall" type="checkbox" checked={showAll} onChange={(e) => toggle(e.currentTarget.checked)} />
            Show settings this device cannot use
          </label>
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
          {lacking ? (
            <div class="soon" role="status" id="lacking">
              <p>{lacking}</p>
            </div>
          ) : null}
          {s.settings ? <View key={s.settingsRev} s={s.settings} /> : <p class="empty">Loading the settings.</p>}
        </section>
      </div>
    </div>
  );
}
