import type { JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { reminderBody } from "../forms.ts";
import { fail, say, usePolled, zone } from "../store.ts";
import { DAYS, fmt, hhmm, parts, repeatText, span, whenText, zoneName } from "../time.ts";
import type { ScheduleItem, ScheduleKind, ScheduleList } from "../types.ts";

const NAMES: Record<ScheduleKind, string> = { timer: "Timer", alarm: "Alarm", reminder: "Reminder" };
const PLACEHOLDER: Record<ScheduleKind, string> = {
  timer: "For example: pasta",
  alarm: "For example: wake up",
  reminder: "For example: call Mum",
};
const today = () => parts(Math.floor(Date.now() / 1000), zone()).date;

function whenLine(i: ScheduleItem, now: number): string {
  const z = zone();
  if (i.kind === "timer") return `Rings in ${span(i.seconds_left)} (${whenText(i.fire_at, now, z)})`;
  if (i.repeat)
    return `${repeatText(i.repeat)} at ${fmt(i.fire_at, z, { hour: "numeric", minute: "2-digit" })}. Next: ${whenText(i.fire_at, now, z)}`;
  return whenText(i.fire_at, now, z);
}

export function Reminders(): JSX.Element {
  const [data, setData] = useState<ScheduleList | null>(null);
  const [unavailable, setUnavailable] = useState("");
  const [editing, setEditing] = useState(0);
  const [kind, setKind] = useState<ScheduleKind>("timer");
  const [label, setLabel] = useState("");
  const [amount, setAmount] = useState("10");
  const [unit, setUnit] = useState("60");
  const [time, setTime] = useState("07:00");
  const [days, setDays] = useState<number[]>([]);
  const [date, setDate] = useState(today());
  const [time2, setTime2] = useState("09:00");
  const [focusTick, setFocusTick] = useState(0);
  const formRef = useRef<HTMLFormElement>(null);

  usePolled(async () => {
    try {
      setData(await api<ScheduleList>("GET", "/api/schedule"));
      setUnavailable("");
    } catch (x) {
      if (x instanceof ApiError && x.status === 503) setUnavailable(x.message);
      else fail(x);
    }
  });

  // After "Edit": bring the form into view and put the cursor in the name box.
  useEffect(() => {
    if (!focusTick) return;
    formRef.current?.scrollIntoView({ block: "center" });
    document.getElementById("rlabel")?.focus();
  }, [focusTick]);

  const reset = () => {
    setEditing(0);
    setKind("timer");
    setLabel("");
    setAmount("10");
    setUnit("60");
    setTime("07:00");
    setDays([]);
    setDate(today());
    setTime2("09:00");
  };

  const submit = async (ev: Event) => {
    ev.preventDefault();
    const body = reminderBody({ kind, label, amount, unit, time, days, date, time2 });
    if (typeof body === "string") {
      say(body, true);
      return;
    }
    try {
      const r = editing
        ? await api<ScheduleList>("PUT", "/api/schedule/" + editing, body)
        : await api<ScheduleList>("POST", "/api/schedule", body);
      say(editing ? "Saved." : "Added.");
      reset();
      setData(r);
    } catch (x) {
      fail(x);
    }
  };

  const startEdit = (i: ScheduleItem) => {
    setEditing(i.id);
    setKind(i.kind);
    setLabel(i.label);
    if (i.kind === "timer") {
      setAmount(String(Math.max(1, Math.ceil(i.seconds_left / 60))));
      setUnit("60");
    } else if (i.kind === "alarm") {
      setTime(i.repeat ? hhmm(i.repeat.hour, i.repeat.minute) : parts(i.fire_at, zone()).time);
      setDays(i.repeat ? i.repeat.days : []);
    } else {
      const q = parts(i.fire_at, zone());
      setDate(q.date);
      setTime2(q.time);
    }
    setFocusTick((n) => n + 1);
  };

  const cancel = async (i: ScheduleItem) => {
    if (!confirm("Cancel this " + i.kind + (i.label ? " (" + i.label + ")" : "") + "?")) return;
    try {
      setData(await api<ScheduleList>("DELETE", "/api/schedule/" + i.id));
      say("Cancelled.");
    } catch (x) {
      fail(x);
    }
  };

  const note =
    unavailable ||
    (data
      ? (data.clock_valid ? "" : "The device clock is not set yet, so nothing will ring until it is. ") +
        "Times are in " +
        zoneName(zone()) +
        ", the device's time zone."
      : "");

  return (
    <>
      <h2>Reminders</h2>
      <p>
        Timers, alarms, and reminders you set here or by voice. They ring on the speaker even when the internet is down.
      </p>
      <ul class="rows">
        {data && !data.items.length ? (
          <li class="mut">Nothing set. Add a timer, alarm, or reminder below, or just ask the speaker.</li>
        ) : null}
        {data?.items.map((i) => (
          <li key={i.id}>
            <div class="rowhead">
              <strong>{i.label || NAMES[i.kind]}</strong>
              <span class="badge">{NAMES[i.kind]}</span>
            </div>
            <div class="when">{whenLine(i, data.now)}</div>
            <div class="acts">
              <button class="plain" type="button" onClick={() => startEdit(i)}>
                Edit
              </button>
              <button class="danger" type="button" onClick={() => cancel(i)}>
                Cancel
              </button>
            </div>
          </li>
        ))}
      </ul>
      <p class="mut">{note}</p>
      <form class="panel" ref={formRef} onSubmit={submit}>
        <h3>{editing ? "Change this " + kind : "Add a timer, alarm, or reminder"}</h3>
        <label for="rkind">Type</label>
        <select
          id="rkind"
          aria-label="What to add"
          value={kind}
          onChange={(e) => setKind(e.currentTarget.value as ScheduleKind)}
        >
          <option value="timer">Timer (counts down)</option>
          <option value="alarm">Alarm (a time of day)</option>
          <option value="reminder">Reminder (a date and a message)</option>
        </select>
        {kind === "timer" ? (
          <div id="rtimer">
            <label for="ramount">Length</label>
            <div class="row">
              <input
                id="ramount"
                type="number"
                min="1"
                max="10080"
                value={amount}
                onInput={(e) => setAmount(e.currentTarget.value)}
              />
              <select id="runit" aria-label="Unit" value={unit} onChange={(e) => setUnit(e.currentTarget.value)}>
                <option value="60">minutes</option>
                <option value="3600">hours</option>
              </select>
            </div>
          </div>
        ) : null}
        {kind === "alarm" ? (
          <div id="ralarm">
            <label for="rtime">Time</label>
            <input id="rtime" type="time" value={time} onInput={(e) => setTime(e.currentTarget.value)} />
            <label>Repeat on</label>
            <fieldset class="days">
              <legend class="sr">Repeat on</legend>
              {DAYS.map((d, i) => (
                <label key={d}>
                  <input
                    type="checkbox"
                    value={i}
                    checked={days.includes(i)}
                    onChange={(e) => setDays(e.currentTarget.checked ? [...days, i] : days.filter((x) => x !== i))}
                  />
                  {d}
                </label>
              ))}
            </fieldset>
            <p class="mut">Leave every day off to ring once, at the next time it comes round.</p>
          </div>
        ) : null}
        {kind === "reminder" ? (
          <div id="rrem">
            <label for="rdate">Date</label>
            <input id="rdate" type="date" value={date} onInput={(e) => setDate(e.currentTarget.value)} />
            <label for="rtime2">Time</label>
            <input id="rtime2" type="time" value={time2} onInput={(e) => setTime2(e.currentTarget.value)} />
          </div>
        ) : null}
        <label for="rlabel">{kind === "reminder" ? "What should it say?" : "Name (optional)"}</label>
        <input
          id="rlabel"
          type="text"
          maxLength={100}
          placeholder={PLACEHOLDER[kind]}
          value={label}
          onInput={(e) => setLabel(e.currentTarget.value)}
        />
        <div class="row">
          <button>{editing ? "Save changes" : "Add"}</button>
          {editing ? (
            <button class="plain" type="button" onClick={reset}>
              Stop editing
            </button>
          ) : null}
        </div>
      </form>
    </>
  );
}
