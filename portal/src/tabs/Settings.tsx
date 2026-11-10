import type { JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { currentText, FROM, labelOf, settingsPatch, type SettingSpec } from "../forms.ts";
import { fail, get, say, set } from "../store.ts";
import { zoneName, zoneOptions } from "../time.ts";
import type { Settings as SettingsData } from "../types.ts";
import { languageOptions } from "../languages.ts";
import { VoiceControl } from "./Voice.tsx";

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
export function Group(props: {
  title: string;
  intro?: string;
  specs: SettingSpec[];
  settings: SettingsData;
  /** Extra controls shown above the rows, given the form's values and a way to change them. */
  extra?: (f: { vals: Record<string, string>; setVals: (v: Record<string, string>) => void }) => JSX.Element;
}): JSX.Element {
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
  // Optional settings start hidden unless they already hold a value.
  const [shown, setShown] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(specs.filter((sp) => sp.optional).map((sp) => [sp.key, (start0[sp.key] ?? "") !== ""])),
  );
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

  const saved = specs.filter((sp) => settings.fields[sp.key]?.saved);
  const reset = async () => {
    if (!confirm("Put " + title.toLowerCase() + " back to the defaults? Values saved here are removed.")) return;
    try {
      const next = await api<SettingsData>(
        "PUT",
        "/api/settings",
        Object.fromEntries(saved.map((sp) => [sp.key, null])),
      );
      const merged = Object.assign({}, get().settings, next);
      set({ settings: merged, settingsRev: get().settingsRev + 1 });
      say("Back to the defaults.");
    } catch (x) {
      fail(x);
    }
  };

  return (
    <form class="panel card sgroup" onSubmit={submit}>
      <div class="head">
        <h3>{title}</h3>
        {saved.length ? (
          <button class="plain small" type="button" onClick={reset}>
            Reset to defaults
          </button>
        ) : null}
      </div>
      {intro ? <p class="mut">{intro}</p> : null}
      {props.extra ? props.extra({ vals, setVals }) : null}
      {specs.map((sp) => {
        const f = settings.fields[sp.key];
        if (!f || sp.hideWhen?.(vals)) return null;
        if (sp.optional && !shown[sp.key]) return null;
        const id = "f-" + sp.key;
        const v = vals[sp.key] ?? "";
        const input = (e: Event) => setVals({ ...vals, [sp.key]: (e.currentTarget as HTMLInputElement).value });
        let ctl;
        if (sp.kind === "zone") {
          ctl = zs ? (
            <select id={id} value={v} onChange={input}>
              {zoneOptions(zones).map((z) => (
                <option key={z.id} value={z.id}>
                  {z.label}
                </option>
              ))}
            </select>
          ) : (
            <input id={id} type="text" value={v} placeholder={bz || "Europe/Berlin"} onInput={input} />
          );
        } else if (sp.kind === "language") {
          ctl = (
            <select id={id} value={v} disabled={sp.off} onChange={input}>
              <option value="">Detect it automatically</option>
              {languageOptions(v).map((l) => (
                <option key={l.code} value={l.code}>
                  {l.label}
                </option>
              ))}
            </select>
          );
        } else if (sp.kind === "voice") {
          ctl = (
            <VoiceControl
              id={id}
              value={v}
              saved={current[sp.key] ?? ""}
              off={sp.off}
              onInput={(x) => setVals({ ...vals, [sp.key]: x })}
            />
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
          ctl = (
            <input
              id={id}
              type="text"
              value={v}
              autoComplete="off"
              list={sp.datalist}
              disabled={sp.off}
              placeholder={sp.off ? "Not available yet" : undefined}
              onInput={input}
            />
          );
        }
        return (
          <div class="srow" key={sp.key}>
            <div class="slabel">
              <label for={id}>
                {labelOf(sp.key)}
                {sp.optional ? <span class="optional">Optional</span> : null}
              </label>
              {hints[sp.key] ? <p class="mut">{hints[sp.key]}</p> : null}
              {f.source !== "default" || !f.live || f.ignored ? (
                <p class="meta">
                  {[
                    f.source !== "default" ? (FROM[f.source] ?? f.source) : "",
                    f.live ? "" : "Needs a restart",
                    f.ignored
                      ? "A command-line flag is set, so a value saved here has no effect until the flag is removed"
                      : "",
                  ]
                    .filter(Boolean)
                    .join(". ") + "."}
                </p>
              ) : null}
            </div>
            <div class="sctl">
              {ctl}
              {sp.optional ? (
                <button
                  class="plain small"
                  type="button"
                  onClick={() => {
                    setVals({ ...vals, [sp.key]: "" });
                    setShown({ ...shown, [sp.key]: false });
                  }}
                >
                  Remove
                </button>
              ) : null}
            </div>
          </div>
        );
      })}
      {specs.some((sp) => sp.optional && !shown[sp.key]) ? (
        <div class="addrow">
          {specs
            .filter((sp) => sp.optional && !shown[sp.key])
            .map((sp) => (
              <button
                key={sp.key}
                class="plain small addopt"
                type="button"
                onClick={() => {
                  setShown({ ...shown, [sp.key]: true });
                  setTimeout(() => document.getElementById("f-" + sp.key)?.focus(), 0);
                }}
              >
                <span aria-hidden="true">+</span> {sp.optional}
              </button>
            ))}
          <span class="mut">Optional</span>
        </div>
      ) : null}
      <p class="errtext" role="alert" ref={errRef}>
        {err}
      </p>
      <div class="row">
        <button>{"Save " + title.toLowerCase()}</button>
      </div>
    </form>
  );
}
