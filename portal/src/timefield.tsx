// A time of day you can plainly type: an hour box, a minute box and AM or PM, instead of the browser's own time box,
// whose small parts do not look editable. The value is always "HH:MM" on a 24-hour clock, or "" for no time.
import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";

/** Whether this person's browser shows 12-hour clocks. */
export function wants12(): boolean {
  try {
    return new Intl.DateTimeFormat(undefined, { hour: "numeric" }).resolvedOptions().hour12 === true;
  } catch {
    return true;
  }
}

const two = (n: number) => String(n).padStart(2, "0");

/** The boxes' text for a "HH:MM" value. */
export function boxes(value: string, h12: boolean): { h: string; m: string; pm: boolean } {
  const m = /^(\d{1,2}):(\d{2})$/.exec(value);
  if (!m) return { h: "", m: "", pm: false };
  const h24 = Number(m[1]);
  const pm = h24 >= 12;
  return { h: h12 ? String(h24 % 12 === 0 ? 12 : h24 % 12) : two(h24), m: m[2] ?? "", pm };
}

/** "HH:MM" from what is in the boxes, or "" when they do not make a time. */
export function joined(h: string, m: string, pm: boolean, h12: boolean): string {
  if (!/^\d{1,2}$/.test(h) || !/^\d{1,2}$/.test(m)) return "";
  const hh = Number(h);
  const mm = Number(m);
  if (mm > 59) return "";
  if (h12) {
    if (hh < 1 || hh > 12) return "";
    return two((hh % 12) + (pm ? 12 : 0)) + ":" + two(mm);
  }
  if (hh > 23) return "";
  return two(hh) + ":" + two(mm);
}

export function TimeField(props: {
  id: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  /** Offer a button that empties it (for a time that is optional). */
  clearable?: boolean;
}): JSX.Element {
  const h12 = wants12();
  const start = boxes(props.value, h12);
  const [h, setH] = useState(start.h);
  const [m, setM] = useState(start.m);
  const [pm, setPm] = useState(start.pm);
  // Follow the value when it is changed from outside (editing an item, clearing, saving).
  useEffect(() => {
    if (joined(h, m, pm, h12) === props.value) return;
    const b = boxes(props.value, h12);
    setH(b.h);
    setM(b.m);
    setPm(b.pm);
    // Only an outside change of the value should reset the boxes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.value]);

  const emit = (nh: string, nm: string, np: boolean) => {
    const v = joined(nh, nm, np, h12);
    if (v || (!nh && !nm)) props.onChange(v);
  };
  const digits = (s: string) => s.replace(/\D/g, "").slice(0, 2);
  const step = (ev: KeyboardEvent, cur: string, lo: number, hi: number, set: (v: string) => void, which: "h" | "m") => {
    if (ev.key !== "ArrowUp" && ev.key !== "ArrowDown") return;
    ev.preventDefault();
    const n = Number(cur || (ev.key === "ArrowUp" ? lo - 1 : hi + 1));
    const next = ev.key === "ArrowUp" ? (n >= hi ? lo : n + 1) : n <= lo ? hi : n - 1;
    const text = which === "m" || !h12 ? two(next) : String(next);
    set(text);
    emit(which === "h" ? text : h, which === "m" ? text : m, pm);
  };
  const hourRange: [number, number] = h12 ? [1, 12] : [0, 23];

  return (
    <div class="timefield" role="group" aria-label="Time">
      <input
        id={props.id}
        type="text"
        inputMode="numeric"
        autoComplete="off"
        maxLength={2}
        placeholder="--"
        aria-label="Hour"
        disabled={props.disabled}
        value={h}
        onFocus={(e) => e.currentTarget.select()}
        onInput={(e) => {
          const t = digits(e.currentTarget.value);
          setH(t);
          emit(t, m, pm);
          if (t.length === 2) document.getElementById(props.id + "-m")?.focus();
        }}
        onKeyDown={(e) => step(e, h, hourRange[0], hourRange[1], setH, "h")}
      />
      <span class="timecolon" aria-hidden="true">
        :
      </span>
      <input
        id={props.id + "-m"}
        type="text"
        inputMode="numeric"
        autoComplete="off"
        maxLength={2}
        placeholder="--"
        aria-label="Minute"
        disabled={props.disabled}
        value={m}
        onFocus={(e) => e.currentTarget.select()}
        onInput={(e) => {
          const t = digits(e.currentTarget.value);
          setM(t);
          emit(h, t, pm);
        }}
        onBlur={() => {
          if (m.length === 1) setM(two(Number(m)));
        }}
        onKeyDown={(e) => step(e, m, 0, 59, setM, "m")}
      />
      {h12 ? (
        <select
          id={props.id + "-p"}
          aria-label="AM or PM"
          disabled={props.disabled}
          value={pm ? "pm" : "am"}
          onChange={(e) => {
            const np = e.currentTarget.value === "pm";
            setPm(np);
            emit(h, m, np);
          }}
        >
          <option value="am">AM</option>
          <option value="pm">PM</option>
        </select>
      ) : null}
      {props.clearable && props.value ? (
        <button
          class="plain small"
          type="button"
          onClick={() => {
            setH("");
            setM("");
            props.onChange("");
          }}
        >
          Clear
        </button>
      ) : null}
    </div>
  );
}
