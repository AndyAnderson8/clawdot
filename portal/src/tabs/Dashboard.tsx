import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import {
  attention,
  isRefusal,
  lastHeard,
  recentItems,
  refusal,
  speakBlock,
  speakerView,
  type SpeakBlock,
} from "../dashboard.ts";
import { reminderBody } from "../forms.ts";
import { fail, load, say, useApp, usePolled, zone } from "../store.ts";
import { ago, fmt, whenText } from "../time.ts";
import type { AuditPage, AuditRecord, ScheduleItem, ScheduleList } from "../types.ts";
import { NAMES, whenLine } from "./Reminders.tsx";

function Ring(props: { state: string }): JSX.Element {
  return (
    <svg class="ring" data-state={props.state} viewBox="0 0 52 32" aria-hidden="true">
      <ellipse cx="26" cy="16" rx="22" ry="10" fill="none" stroke="currentColor" stroke-width="4" />
    </svg>
  );
}

/** Say a phrase out loud, or say plainly why it cannot be done and where to fix that. */
function SayIt(): JSX.Element {
  const s = useApp();
  const [text, setText] = useState("");
  const [refused, setRefused] = useState<SpeakBlock | null>(null);
  const known = speakBlock(s.status);
  const block = known ?? refused;

  const speak = async (ev: Event) => {
    ev.preventDefault();
    setRefused(null);
    try {
      await api("POST", "/api/speak", { text });
      setText("");
      say("Sent. Listen for the speaker.");
      void load();
    } catch (x) {
      if (x instanceof ApiError && isRefusal(x.reason)) setRefused(refusal(x.reason));
      else fail(x);
    }
  };
  return (
    <form id="sayform" class="d-say" onSubmit={speak}>
      <h3>Say something</h3>
      <p class="mut">Type a sentence and the speaker says it out loud.</p>
      <label for="speaktext" class="sr">
        What the speaker should say
      </label>
      <div class="row">
        <input
          id="speaktext"
          type="text"
          placeholder="Hello, is anyone home?"
          maxLength={200}
          disabled={!!known}
          value={text}
          onInput={(e) => setText(e.currentTarget.value)}
        />
        <button id="saybtn" disabled={!!known}>
          Say it
        </button>
      </div>
      {block ? (
        <p class="errtext" id="sayblock" role="status">
          {block.text}{" "}
          {block.to ? (
            <a href={block.to} id="sayfix">
              {block.label}
            </a>
          ) : null}
        </p>
      ) : null}
      <div class="row">
        <button class="plain" type="button" disabled>
          Talk now
        </button>
        <button class="plain" type="button" disabled>
          Mute
        </button>
      </div>
      <p class="mut">
        The page cannot start listening or mute the speaker yet. Say the wake word, or use the speaker's mute button.
      </p>
    </form>
  );
}

/** A timer in two clicks; alarms and reminders with a day or a date are on the Reminders page. */
function QuickAdd(props: { onAdded: (l: ScheduleList) => void }): JSX.Element {
  const [amount, setAmount] = useState("10");
  const [unit, setUnit] = useState("60");
  const [label, setLabel] = useState("");
  const add = async (ev: Event) => {
    ev.preventDefault();
    const body = reminderBody({ kind: "timer", label, amount, unit, time: "", days: [], date: "", time2: "" });
    if (typeof body === "string") {
      say(body, true);
      return;
    }
    try {
      props.onAdded(await api<ScheduleList>("POST", "/api/schedule", body));
      say("Added.");
      setLabel("");
    } catch (x) {
      fail(x);
    }
  };
  return (
    <form id="quickadd" onSubmit={add}>
      <label for="qamount">Add a timer</label>
      <div class="row">
        <input
          id="qamount"
          type="number"
          min="1"
          max="10080"
          value={amount}
          onInput={(e) => setAmount(e.currentTarget.value)}
        />
        <select id="qunit" aria-label="Unit" value={unit} onChange={(e) => setUnit(e.currentTarget.value)}>
          <option value="60">minutes</option>
          <option value="3600">hours</option>
        </select>
        <input
          id="qlabel"
          type="text"
          maxLength={100}
          placeholder="Name (optional)"
          aria-label="Name"
          value={label}
          onInput={(e) => setLabel(e.currentTarget.value)}
        />
        <button>Add timer</button>
      </div>
      <p class="mut">
        For an alarm or a reminder, use <a href="#/reminders">Reminders</a>.
      </p>
    </form>
  );
}

export function Dashboard(): JSX.Element {
  const s = useApp();
  const st = s.status;
  const [sched, setSched] = useState<ScheduleList | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [records, setRecords] = useState<AuditRecord[] | null>(null);

  usePolled(async () => {
    try {
      setSched(await api<ScheduleList>("GET", "/api/schedule"));
      setUnavailable(false);
    } catch (x) {
      if (x instanceof ApiError && x.status === 503) setUnavailable(true);
      else fail(x);
    }
    try {
      setRecords((await api<AuditPage>("GET", "/api/audit?limit=20")).records);
    } catch (x) {
      // No activity log on this device is a normal state, not a failure.
      if (x instanceof ApiError && x.status === 503) setRecords([]);
      else fail(x);
    }
  });

  const now = s.overview?.now ?? Math.floor(Date.now() / 1000);
  const view = speakerView(st);
  const heard = records ? lastHeard(records) : null;
  const detail = view.detail || (heard ? `Last heard ${ago(heard, now)}.` : records ? "Nothing heard yet." : "");
  const todo = attention(st, s.mode);
  const next = (sched?.items ?? []).slice(0, 5);
  const recent = recentItems(records ?? [], s.overview?.events ?? []);
  const u = st?.usage;
  const model = st?.llm?.model;

  const cancel = async (i: ScheduleItem) => {
    if (!confirm("Cancel this " + i.kind + (i.label ? " (" + i.label + ")" : "") + "?")) return;
    try {
      setSched(await api<ScheduleList>("DELETE", "/api/schedule/" + i.id));
      say("Cancelled.");
    } catch (x) {
      fail(x);
    }
  };

  return (
    <div class="dash">
      <div class="col">
        <section class="status d-status" id="statusblock">
          <Ring state={view.ring} />
          <div role="status">
            <h2 id="statusline">{view.headline}</h2>
            {detail ? <p id="statusdetail">{detail}</p> : null}
          </div>
        </section>

        <section class="sect d-attention" id="attention">
          <h3>Needs attention</h3>
          {st?.llm ? (
            todo.length ? (
              <ul class="rows">
                {todo.map((a) => (
                  <li key={a.id}>
                    <div class="fix">
                      <div>
                        <strong>{a.title}</strong>
                        <div class="mut">{a.why}</div>
                      </div>
                      {a.to ? <a href={a.to}>{a.label}</a> : null}
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <p class="empty">
                <span class="dot ok" aria-hidden="true" />
                Nothing needs attention.
              </p>
            )
          ) : null}
        </section>

        <section class="sect d-next" id="nextup">
          <div class="head">
            <h3>Next up</h3>
            <a href="#/reminders">All reminders</a>
          </div>
          {unavailable ? (
            <p class="empty">Timers and reminders are not running on this device.</p>
          ) : (
            <>
              <ul class="rows">
                {sched && !next.length ? (
                  <li class="empty">Nothing is set. Add a timer below, or ask the speaker.</li>
                ) : null}
                {next.map((i) => (
                  <li key={i.id}>
                    <div class="rowhead">
                      <strong>{i.label || NAMES[i.kind]}</strong>
                      <span class="badge">{NAMES[i.kind]}</span>
                    </div>
                    <div class="when">{whenLine(i, sched?.now ?? now)}</div>
                    <div class="acts">
                      <button class="danger" type="button" onClick={() => cancel(i)}>
                        Cancel
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
              <QuickAdd onAdded={setSched} />
            </>
          )}
        </section>

        <section class="sect d-recent" id="recent">
          <div class="head">
            <h3>Recent</h3>
            <a href="#/activity">All activity</a>
          </div>
          <ul class="rows">
            {records && !recent.length ? (
              <li class="empty">
                Nothing yet. When the assistant does something, or an input sends an event, it shows here.
              </li>
            ) : null}
            {recent.map((r) => (
              <li key={r.key}>
                <div class="rowhead">
                  <span>
                    <span class="num mut">{whenText(r.ts, now, zone())}</span> {r.title}
                  </span>
                  <span class={"badge" + (r.tone ? " " + r.tone : "")}>{r.note}</span>
                </div>
              </li>
            ))}
          </ul>
        </section>
      </div>

      <div class="side">
        <SayIt />
        <section class="sect d-today">
          <h3>Today</h3>
          {u ? (
            <dl class="tiny">
              <dt>Model requests</dt>
              <dd>
                {u.requests_today} of {u.max_requests_per_day}
              </dd>
              {u.spend_today > 0 ? (
                <>
                  <dt>Spent</dt>
                  <dd>${u.spend_today.toFixed(3)}</dd>
                </>
              ) : null}
              {model ? (
                <>
                  <dt>Model</dt>
                  <dd>{model}</dd>
                </>
              ) : null}
              <dt>Time now</dt>
              <dd>{fmt(now, zone(), { hour: "numeric", minute: "2-digit" })}</dd>
            </dl>
          ) : (
            <p class="empty">Checking.</p>
          )}
        </section>
      </div>
    </div>
  );
}
