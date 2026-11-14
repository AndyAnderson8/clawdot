import type { JSX } from "preact";
import { useMemo, useRef, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { currentText, FROM, hintText, labelOf, settingsPatch, type SettingSpec, cents } from "../forms.ts";
import { providerFor } from "../providers.ts";
import { TimeField } from "../timefield.tsx";
import { attempt, adopt, fail, say, useApp } from "../store.ts";
import { zoneName, zoneOptions } from "../time.ts";
import type { SettingField, Settings as SettingsData } from "../types.ts";
import { languageOptions } from "../languages.ts";
import { ModelSelect, VoiceControl } from "./Voice.tsx";
import { ask } from "../confirm.tsx";
import { useBusy } from "../parts.tsx";

function browserZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch {
    return "";
  }
}
let zonesKnown: string[] | null | undefined;
/** Every time zone the browser knows, listed once. Null where it cannot list them. */
function zoneList(): string[] | null {
  if (zonesKnown !== undefined) return zonesKnown;
  zonesKnown = null;
  try {
    const l = Intl.supportedValuesOf("timeZone");
    if (l.length) zonesKnown = ["UTC", ...l];
  } catch {
    /* an older browser: fall back to a text box */
  }
  return zonesKnown;
}

/**
 * The input for one setting, drawn from its kind and limits as the device reports them. It only shows a value and
 * reports a new one; the form keeps the values.
 */
function Control(props: {
  sp: SettingSpec;
  field: SettingField;
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
  /** A time is half typed, so "no time" would be a wrong reading of the box. */
  unfinished: (yes: boolean) => void;
}): JSX.Element {
  const { sp, field, id, saved, put, url, hasKey, keyRev, zones, browserZone, unfinished } = props;
  const v = props.value;
  const zoneChoices = useMemo(() => (field.kind === "zone" && zones ? zoneOptions(zones) : []), [field.kind, zones]);
  const languages = useMemo(() => (field.kind === "language" ? languageOptions(v) : []), [field.kind, v]);
  const input = (e: Event) => put((e.currentTarget as HTMLInputElement).value);
  if (sp.widget === "model") {
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
  if (sp.widget === "voice") {
    return <VoiceControl id={id} value={v} off={sp.off} onInput={put} />;
  }
  if (field.kind === "zone") {
    return zones ? (
      <select id={id} value={v} onChange={input}>
        {zoneChoices.map((z) => (
          <option key={z.id} value={z.id}>
            {z.label}
          </option>
        ))}
      </select>
    ) : (
      <input id={id} type="text" value={v} placeholder={browserZone || "Europe/Berlin"} onInput={input} />
    );
  }
  if (field.kind === "language") {
    return (
      <select id={id} value={v} disabled={sp.off} onChange={input}>
        <option value="">Detect it automatically</option>
        {languages.map((l) => (
          <option key={l.code} value={l.code}>
            {l.label}
          </option>
        ))}
      </select>
    );
  }
  if (field.kind === "long_text") {
    return <textarea id={id} rows={7} maxLength={field.max_length} value={v} onInput={input} />;
  }
  if (field.kind === "choice") {
    return (
      <select id={id} value={v} disabled={sp.off} onChange={input}>
        {(field.choices ?? []).map((val) => (
          <option key={val} value={val}>
            {sp.labels?.[val] ?? val}
          </option>
        ))}
      </select>
    );
  }
  if (field.kind === "flag") {
    return (
      <select id={id} value={v} disabled={sp.off} onChange={input}>
        <option value="true">On</option>
        <option value="false">Off</option>
        {saved ? <option value="">Go back to the default</option> : null}
      </select>
    );
  }
  if (field.kind === "time") {
    return (
      <TimeField
        id={id}
        label={labelOf(sp.key)}
        value={v}
        disabled={sp.off}
        clearable
        onChange={(x, partial) => {
          unfinished(partial === true);
          put(x);
        }}
      />
    );
  }
  if (field.kind === "money") {
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
          onBlur={() => {
            if (v && Number.isFinite(Number(v))) put(Number(v).toFixed(2));
          }}
        />
      </div>
    );
  }
  if (field.kind === "whole") {
    return (
      <input
        id={id}
        type="number"
        value={v}
        autoComplete="off"
        step="1"
        min={field.min === undefined ? undefined : String(field.min)}
        max={field.max === undefined ? undefined : String(field.max)}
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

interface GroupProps {
  title: string;
  intro?: string;
  specs: SettingSpec[];
  settings: SettingsData;
  /** Extra controls shown above the rows, given the form's values and a way to change them. */
  extra?: (f: { vals: Record<string, string>; setVals: (v: Record<string, string>) => void }) => JSX.Element;
  /** More rows after the settings, before the Save button (a key, a status line). */
  below?: () => JSX.Element | null;
}

/**
 * One group of settings with its own Save button. Only the values you changed are sent. Saving starts this group
 * again from the saved values; the other groups keep what has been typed in them.
 */
export function Group(props: GroupProps): JSX.Element {
  const [round, setRound] = useState(0);
  return <GroupForm key={round} {...props} onSaved={() => setRound((n) => n + 1)} />;
}

function GroupForm(props: GroupProps & { onSaved: () => void }): JSX.Element {
  const { title, intro, specs, settings } = props;
  const keyRev = useApp().keyRev;
  const current: Record<string, string> = {};
  const start0: Record<string, string> = {};
  const hints: Record<string, string> = {};
  const bz = browserZone();
  const zs = zoneList();
  const zoneSpec = specs.find((sp) => settings.fields[sp.key]?.kind === "zone");
  const zoneNow = zoneSpec ? currentText(settings, zoneSpec.key) : "";
  const zones = useMemo(() => (zs ? (zs.includes(zoneNow) ? zs : [zoneNow, ...zs]) : null), [zs, zoneNow]);
  for (const sp of specs) {
    const f = settings.fields[sp.key];
    if (!f) continue;
    const cur = currentText(settings, sp.key);
    current[sp.key] = cur;
    start0[sp.key] = cur;
    hints[sp.key] = hintText(sp, f);
    if (f.kind === "zone" && zs) {
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
  // The settings whose time boxes hold something that is not yet a time.
  const [halfTimes, setHalfTimes] = useState<Record<string, boolean>>({});
  const errRef = useRef<HTMLParagraphElement>(null);
  const [busy, run] = useBusy();

  const submit = (ev: Event) => {
    ev.preventDefault();
    return run(() => save());
  };
  const save = async () => {
    setErr("");
    const half = specs.find((sp) => halfTimes[sp.key] && !sp.hideWhen?.(vals) && (!sp.optional || shown[sp.key]));
    const body = half
      ? "Finish the time for “" + labelOf(half.key) + "” (an hour and a minute), or empty both boxes."
      : settingsPatch(settings.fields, specs, current, vals);
    if (typeof body === "string") {
      setErr(body);
      errRef.current?.scrollIntoView({ block: "nearest" });
      return;
    }
    if (!Object.keys(body).length) {
      say("Nothing to save: you have not changed anything here.");
      return;
    }
    try {
      const next = await api<SettingsData>("PUT", "/api/settings", body);
      const merged = adopt(next);
      props.onSaved();
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
    await attempt(async () => {
      const next = await api<SettingsData>(
        "PUT",
        "/api/settings",
        Object.fromEntries(saved.map((sp) => [sp.key, null])),
      );
      adopt(next);
      props.onSaved();
      say("Back to the defaults.");
    });
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
                field={f}
                id={id}
                value={vals[sp.key] ?? ""}
                saved={f.saved === true}
                put={(x) => setVals({ ...vals, [sp.key]: x })}
                url={vals["llm_url"] ?? ""}
                hasKey={settings.llm?.key_present === true}
                keyRev={keyRev}
                zones={zones}
                browserZone={bz}
                unfinished={(yes) => setHalfTimes((h) => (h[sp.key] === yes ? h : { ...h, [sp.key]: yes }))}
              />
              {sp.optional ? (
                <button
                  class="plain small"
                  type="button"
                  onClick={() => {
                    setVals({ ...vals, [sp.key]: "" });
                    setShown({ ...shown, [sp.key]: false });
                    setHalfTimes({ ...halfTimes, [sp.key]: false });
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
        <button disabled={busy}>{"Save " + title.charAt(0).toLowerCase() + title.slice(1)}</button>
      </div>
    </form>
  );
}
