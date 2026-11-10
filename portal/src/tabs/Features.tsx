import type { JSX } from "preact";
import { api } from "../api.ts";
import { currentText } from "../forms.ts";
import { fail, get, say, set, useApp } from "../store.ts";
import type { Settings as SettingsData } from "../types.ts";

interface Feature {
  key: string;
  title: string;
  text: string;
  /** SVG path for the icon. */
  icon: string;
  more?: { href: string; label: string };
}

/** Abilities you can switch on and off at any time. Things you set once (the model, the voice, the inputs) are in
 *  Settings; these are choices about how the assistant behaves. */
const FEATURES: Feature[] = [
  {
    key: "notes",
    title: "Remember things",
    text: "Say “remember that my printer is called Pixel” and it knows from then on.",
    icon: "M5 4h11a3 3 0 0 1 3 3v13H8a3 3 0 0 1-3-3Zm0 13a3 3 0 0 1 3-3h11",
    more: { href: "#/settings/memory", label: "See what it remembers" },
  },
  {
    key: "plan_speech",
    title: "Say the plan first",
    text: "Before a request with several steps, it tells you what it is about to do.",
    icon: "M4 6h16M4 12h10M4 18h6",
  },
  {
    key: "progress_speech",
    title: "Say what it is doing",
    text: "Short phrases such as “Checking the lights” while a tool runs, so you are not left waiting in silence.",
    icon: "M3 12h4l3-8 4 16 3-8h4",
  },
  {
    key: "dry_run",
    title: "Practice mode",
    text: "It only says what it would do, and nothing changes. Good for trying out a new tool.",
    icon: "M12 3 4 7v5c0 5 3.500 8 8 9 4.500-1 8-4 8-9V7Z",
    more: { href: "#/settings/safety", label: "Safety limits" },
  },
];

function FeatureCard(props: { f: Feature; s: SettingsData }): JSX.Element | null {
  const { f, s } = props;
  const field = s.fields[f.key];
  if (!field) return null;
  const on = currentText(s, f.key) === "true";
  const toggle = async (ev: Event) => {
    const next = (ev.currentTarget as HTMLInputElement).checked;
    try {
      const saved = await api<SettingsData>("PUT", "/api/settings", { [f.key]: next });
      set({ settings: Object.assign({}, get().settings, saved), settingsRev: get().settingsRev + 1 });
      say(f.title + (next ? " is on." : " is off."));
    } catch (x) {
      fail(x);
    }
  };
  const id = "feat-" + f.key;
  return (
    <section class="card feature" id={"card-" + f.key}>
      <div class="top">
        <div class="titlebox">
          <span class="fico" aria-hidden="true">
            <svg class="ico" viewBox="0 0 24 24">
              <path d={f.icon} />
            </svg>
          </span>
          <h3>
            <label for={id}>{f.title}</label>
          </h3>
        </div>
        <input id={id} class="switch" type="checkbox" checked={on} onChange={toggle} aria-describedby={id + "-t"} />
      </div>
      <p class="mut" id={id + "-t"}>
        {f.text}
      </p>
      {f.more ? (
        <a class="more" href={f.more.href}>
          {f.more.label}
        </a>
      ) : null}
    </section>
  );
}

export function Features(): JSX.Element {
  const s = useApp();
  const st = s.settings;
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Features</h2>
        <p>Switch abilities on or off. Changes apply right away.</p>
      </header>
      {st ? (
        <div class="features">
          {FEATURES.map((f) => (
            <FeatureCard key={f.key} f={f} s={st} />
          ))}
        </div>
      ) : (
        <p class="empty">Loading.</p>
      )}
    </div>
  );
}
