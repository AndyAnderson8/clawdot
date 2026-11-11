import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { reminderBody } from "../forms.ts";
import { fail, say, usePolled, zone } from "../store.ts";
import { TimeField } from "../timefield.tsx";
import { AddButton, Dialog } from "../dialog.tsx";
import { DAYS, fmt, hhmm, parts, repeatText, span, whenText, zoneName } from "../time.ts";
import type { ScheduleItem, ScheduleKind, ScheduleList } from "../types.ts";

export const NAMES: Record<ScheduleKind, string> = { timer: "Timer", alarm: "Alarm", reminder: "Reminder" };
const PLACEHOLDER: Record<ScheduleKind, string> = {
  timer: "For example: pasta",
  alarm: "For example: wake up",
  reminder: "For example: call the dentist",
};
const PLURAL: Record<ScheduleKind, string> = { timer: "Timers", alarm: "Alarms", reminder: "Reminders" };
const EMPTY: Record<ScheduleKind, string> = {
  timer: "No timers running. Add one below, or ask the speaker.",
  alarm: "No alarms set. Add one below, or ask the speaker.",
  reminder: "No reminders set. Add one below, or ask the speaker.",
};
const ADD: Record<ScheduleKind, string> = { timer: "Add a timer", alarm: "Add an alarm", reminder: "Add a reminder" };
const today = () => parts(Math.floor(Date.now() / 1000), zone()).date;

export function whenLine(i: ScheduleItem, now: number): string {
  const z = zone();
  if (i.kind === "timer") return `Rings in ${span(i.seconds_left)} (${whenText(i.fire_at, now, z)})`;
  if (i.repeat)
    return `${repeatText(i.repeat)} at ${fmt(i.fire_at, z, { hour: "numeric", minute: "2-digit" })}. Next: ${whenText(i.fire_at, now, z)}`;
  return whenText(i.fire_at, now, z);
}

function KindPage(props: { kind: ScheduleKind }): JSX.Element {
  const [data, setData] = useState<ScheduleList | null>(null);
  const [unavailable, setUnavailable] = useState("");
  const [editing, setEditing] = useState(0);
  const kind = props.kind;
  const [label, setLabel] = useState("");
  const [amount, setAmount] = useState("10");
  const [unit, setUnit] = useState("60");
  const [time, setTime] = useState("07:00");
  const [days, setDays] = useState<number[]>([]);
  const [date, setDate] = useState(today());
  const [time2, setTime2] = useState("09:00");
  const [open, setOpen] = useState(false);

  usePolled(async () => {
    try {
      setData(await api<ScheduleList>("GET", "/api/schedule"));
      setUnavailable("");
    } catch (x) {
      if (x instanceof ApiError && x.status === 503) setUnavailable(x.message);
      else fail(x);
    }
  });

  const reset = () => {
    setOpen(false);
    setEditing(0);
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
    setOpen(true);
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

  const here = data?.items.filter((i) => i.kind === kind) ?? [];
  const noun = NAMES[kind].toLowerCase();
  return (
    <div class="stack">
      <section class="card" id="clocklist">
        <div class="head">
          <h3>
            {PLURAL[kind]}
            <span class="count">{here.length}</span>
          </h3>
          <AddButton
            label={ADD[kind]}
            id="addschedule"
            onClick={() => {
              reset();
              setOpen(true);
            }}
          />
        </div>
        <ul class="rows">
          {data && !here.length ? <li class="empty">{EMPTY[kind]}</li> : null}
          {here.map((i) => (
            <li key={i.id}>
              <div class="rowhead">
                <strong>{i.label || NAMES[i.kind]}</strong>
                <span class="badge">{NAMES[i.kind]}</span>
              </div>
              <div class="when">{data ? whenLine(i, data.now) : ""}</div>
              <div class="acts">
                <button class="plain small" type="button" onClick={() => startEdit(i)}>
                  Edit
                </button>
                <button class="danger" type="button" onClick={() => cancel(i)}>
                  Cancel
                </button>
              </div>
            </li>
          ))}
        </ul>
        {note ? <p class="mut chartnote">{note}</p> : null}
      </section>

      <Dialog id="scheduledialog" title={editing ? "Change this " + noun : ADD[kind]} open={open} onClose={reset}>
        <form class="dlgform" onSubmit={submit}>
          {kind === "timer" ? (
            <div class="srow" id="rtimer">
              <div class="slabel">
                <label for="ramount">Length</label>
              </div>
              <div class="sctl">
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
            </div>
          ) : null}
          {kind === "alarm" ? (
            <div id="ralarm">
              <div class="srow">
                <div class="slabel">
                  <label for="rtime">Time</label>
                </div>
                <div class="sctl">
                  <TimeField id="rtime" value={time} onChange={setTime} />
                </div>
              </div>
              <div class="srow">
                <div class="slabel">
                  <label>Repeat on</label>
                  <p class="mut">Leave every day off to ring once, at the next time it comes round.</p>
                </div>
                <div class="sctl">
                  <fieldset class="days">
                    <legend class="sr">Repeat on</legend>
                    {DAYS.map((d, i) => (
                      <label key={d}>
                        <input
                          type="checkbox"
                          value={i}
                          checked={days.includes(i)}
                          onChange={(e) =>
                            setDays(e.currentTarget.checked ? [...days, i] : days.filter((x) => x !== i))
                          }
                        />
                        {d}
                      </label>
                    ))}
                  </fieldset>
                </div>
              </div>
            </div>
          ) : null}
          {kind === "reminder" ? (
            <div id="rrem">
              <div class="srow">
                <div class="slabel">
                  <label for="rdate">Date</label>
                </div>
                <div class="sctl">
                  <input id="rdate" type="date" value={date} onInput={(e) => setDate(e.currentTarget.value)} />
                </div>
              </div>
              <div class="srow">
                <div class="slabel">
                  <label for="rtime2">Time</label>
                </div>
                <div class="sctl">
                  <TimeField id="rtime2" value={time2} onChange={setTime2} />
                </div>
              </div>
            </div>
          ) : null}
          <div class="srow">
            <div class="slabel">
              <label for="rlabel">{kind === "reminder" ? "What should it say?" : "Name (optional)"}</label>
            </div>
            <div class="sctl">
              <input
                id="rlabel"
                type="text"
                maxLength={100}
                placeholder={PLACEHOLDER[kind]}
                value={label}
                onInput={(e) => setLabel(e.currentTarget.value)}
              />
            </div>
          </div>
          <div class="row dlgfoot">
            <button>{editing ? "Save changes" : "Add"}</button>
            <button class="plain" type="button" onClick={reset}>
              Cancel
            </button>
          </div>
        </form>
      </Dialog>
    </div>
  );
}

/** What each page of the Clock is, in the order of its tab strip. The stopwatch is not a schedule item. */
const PAGES: { id: string; label: string; kind?: ScheduleKind; intro: string }[] = [
  { id: "alarms", label: "Alarms", kind: "alarm", intro: "Ring at a time of day, once or on the days you choose." },
  { id: "timers", label: "Timers", kind: "timer", intro: "Count down from a length of time." },
  { id: "reminders", label: "Reminders", kind: "reminder", intro: "Say a message on a date and a time." },
  { id: "stopwatch", label: "Stopwatch", intro: "Count up, and take laps." },
];

/** The stopwatch lives in the assistant, so for now it is used by voice; this page says how. */
function StopwatchPage(): JSX.Element {
  return (
    <div class="stack">
      <section class="card" id="stopwatchcard">
        <h3>Use it by voice</h3>
        <p class="mut">
          The stopwatch counts up from the moment you start it, even across a restart. Ask the speaker; this page does
          not start or stop it yet.
        </p>
        <ul class="rows">
          <li>
            <strong>“Start the stopwatch”</strong>
            <div class="mut">Begins counting.</div>
          </li>
          <li>
            <strong>“Lap”</strong>
            <div class="mut">Notes the time since the last lap.</div>
          </li>
          <li>
            <strong>“How long is the stopwatch at?”</strong>
            <div class="mut">Reads the time without stopping it.</div>
          </li>
          <li>
            <strong>“Stop the stopwatch”</strong>
            <div class="mut">Stops it and says the time and the laps.</div>
          </li>
        </ul>
      </section>
    </div>
  );
}

/** Alarms, timers, reminders and the stopwatch, each on its own page. */
export function Clock(props: { sub: string }): JSX.Element {
  const page = PAGES.find((p) => p.id === props.sub) ?? PAGES[0] ?? { id: "alarms", label: "Alarms", intro: "" };
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Clock</h2>
        <p>{page.intro} They work even when the internet is down.</p>
      </header>
      <nav class="tabs" aria-label="Clock sections" id="clocktabs">
        {PAGES.map((p) => (
          <a key={p.id} href={"#/clock/" + p.id} aria-current={p.id === page.id ? "page" : undefined}>
            {p.label}
          </a>
        ))}
      </nav>
      {page.kind ? <KindPage key={page.id} kind={page.kind} /> : <StopwatchPage />}
    </div>
  );
}
