import type { JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { currentText, FROM, labelOf, settingsPatch, type SettingSpec } from "../forms.ts";
import { fail, get, say, set } from "../store.ts";
import { zoneName } from "../time.ts";
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
export function Group(props: {
  title: string;
  intro?: string;
  specs: SettingSpec[];
  settings: SettingsData;
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
