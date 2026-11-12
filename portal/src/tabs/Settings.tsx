import type { JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { currentText, FROM, labelOf, settingsPatch, type SettingSpec, cents } from "../forms.ts";
import { providerFor } from "../providers.ts";
import { TimeField } from "../timefield.tsx";
import { fail, get, say, set, useApp } from "../store.ts";
import { zoneName, zoneOptions } from "../time.ts";
import type { Settings as SettingsData } from "../types.ts";
import { languageOptions } from "../languages.ts";
import { ModelSelect, VoiceControl } from "./Voice.tsx";
import { ask } from "../confirm.tsx";

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

/** The input for one setting, by its kind. It only shows a value and reports a new one; the form keeps the values. */
function Control(props: {
  sp: SettingSpec;
  id: string;
  value: string;
  /** A value is saved on the device, so a switch can offer to go back to the default. */
  saved: boolean;
  put: (value: string) => void;
  /** The provider address in the form (saved or not), for the list of models. */
  url: string;
  hasKey: boolean;
  keyRev: number;
  /** The time zones to offer, or null where the browser cannot list them. */
  zones: string[] | null;
  browserZone: string;
}): JSX.Element {
  const { sp, id, saved, put, url, hasKey, keyRev, zones, browserZone } = props;
  const v = props.value;
  const input = (e: Event) => put((e.currentTarget as HTMLInputElement).value);
  if (sp.kind === "zone") {
    return zones ? (
      <select id={id} value={v} onChange={input}>
        {zoneOptions(zones).map((z) => (
          <option key={z.id} value={z.id}>
            {z.label}
          </option>
        ))}
      </select>
    ) : (
      <input id={id} type="text" value={v} placeholder={browserZone || "Europe/Berlin"} onInput={input} />
    );
  }
  if (sp.kind === "language") {
    return (
      <select id={id} value={v} disabled={sp.off} onChange={input}>
        <option value="">Detect it automatically</option>
        {languageOptions(v).map((l) => (
          <option key={l.code} value={l.code}>
            {l.label}
          </option>
        ))}
      </select>
    );
  }
  if (sp.kind === "model") {
    // A provider that needs a key is asked for its models only once a key is in place.
    const prov = providerFor(url);
    return (
      <ModelSelect
        id={id}
        value={v}
        url={url}
        rev={String(keyRev)}
        blocked={prov && prov.id !== "custom" && !hasKey ? "Add your API key to see the models" : ""}
        onInput={put}
      />
    );
  }
  if (sp.kind === "voice") {
    return <VoiceControl id={id} value={v} off={sp.off} onInput={put} />;
  }
  if (sp.kind === "area") {
    return <textarea id={id} rows={7} maxLength={4000} value={v} onInput={input} />;
  }
  if (sp.kind === "choice") {
    return (
      <select id={id} value={v} disabled={sp.off} onChange={input}>
        {(sp.options ?? []).map(([val, text]) => (
          <option key={val} value={val}>
            {text}
          </option>
        ))}
      </select>
    );
  }
  if (sp.kind === "bool") {
    return (
      <select id={id} value={v} disabled={sp.off} onChange={input}>
        <option value="true">On</option>
        <option value="false">Off</option>
        {saved ? <option value="">Go back to the default</option> : null}
      </select>
    );
  }
  if (sp.kind === "time") {
    return <TimeField id={id} value={v} disabled={sp.off} clearable onChange={put} />;
  }
  if (sp.kind === "money") {
    return (
      <div class="moneybox">
        <span aria-hidden="true">$</span>
        <input
          id={id}
          type="text"
          inputMode="decimal"
          autoComplete="off"
          placeholder={sp.placeholder ?? "No limit"}
          disabled={sp.off}
          value={v}
          onInput={(e) => put(cents(e.currentTarget.value))}
          onBlur={() => v && put(Number(v || 0).toFixed(2))}
        />
      </div>
    );
  }
  if (sp.kind === "number") {
    const whole = sp.min !== undefined;
    return (
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
  }
  return (
    <input
      id={id}
      type="text"
      value={v}
      autoComplete="off"
      disabled={sp.off}
      placeholder={sp.off ? "Not available yet" : undefined}
      onInput={input}
    />
  );
}

/** One group of settings with its own Save button. Only the values you changed are sent. */
export function Group(props: {
  title: string;
  intro?: string;
  specs: SettingSpec[];
  settings: SettingsData;
  /** Extra controls shown above the rows, given the form's values and a way to change them. */
  extra?: (f: { vals: Record<string, string>; setVals: (v: Record<string, string>) => void }) => JSX.Element;
  /** More rows after the settings, before the Save button (a key, a status line). */
  below?: () => JSX.Element | null;
}): JSX.Element {
  const { title, intro, specs, settings } = props;
  const keyRev = useApp().keyRev;
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
    const sure = await ask(
      "Put " + title.toLowerCase() + " back to the defaults? Values saved here are removed.",
      "Reset them",
    );
    if (!sure) return;
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
                    f.source !== "default" && !f.ignored ? (FROM[f.source] ?? f.source) : "",
                    f.live ? "" : "Applies after a restart",
                    f.ignored ? "Fixed when ClawDot was started, so a value saved here has no effect" : "",
                  ]
                    .filter(Boolean)
                    .join(". ") + "."}
                </p>
              ) : null}
            </div>
            <div class="sctl">
              <Control
                sp={sp}
                id={id}
                value={vals[sp.key] ?? ""}
                saved={f.saved === true}
                put={(x) => setVals({ ...vals, [sp.key]: x })}
                url={vals["llm_url"] ?? ""}
                hasKey={settings.llm?.key_present === true}
                keyRev={keyRev}
                zones={zs ? zones : null}
                browserZone={bz}
              />
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
      {props.below ? props.below() : null}
      <p class="errtext" role="alert" ref={errRef}>
        {err}
      </p>
      <div class="row">
        <button>{"Save " + title.charAt(0).toLowerCase() + title.slice(1)}</button>
      </div>
    </form>
  );
}
