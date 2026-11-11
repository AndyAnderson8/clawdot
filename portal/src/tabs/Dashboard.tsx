import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { attention, dailyCounts, lastHeard, recentItems, speakerView, type Day } from "../dashboard.ts";
import { Dialog } from "../dialog.tsx";
import { fail, say, sv, useApp, usePolled, zone } from "../store.ts";
import { ago, whenText } from "../time.ts";
import type { AuditPage, AuditRecord, UpdateStatus } from "../types.ts";
import { Look, LOOKS, lookPref, photoPref, readPhoto, setLookPref, setPhoto, type LookId } from "./looks.tsx";

/** One number with a plain label. */
function Stat(props: { label: string; value: string; note?: string; id?: string }): JSX.Element {
  return (
    <div class="stat" id={props.id}>
      <div class="statlabel">{props.label}</div>
      <div class="statvalue num">{props.value}</div>
      {props.note ? <div class="statnote">{props.note}</div> : null}
    </div>
  );
}

/** Bars for the last two weeks. The assistant's actions are what the activity log records, so that is what is counted. */
function UsageChart(props: { days: Day[] | null }): JSX.Element {
  const days = props.days;
  const max = Math.max(1, ...(days ?? []).map((d) => d.count));
  const total = (days ?? []).reduce((n, d) => n + d.count, 0);
  return (
    <section class="card" id="usage" aria-labelledby="usage-h">
      <div class="head">
        <h3 id="usage-h">Activity, last 14 days</h3>
        <a href="#/activity">All activity</a>
      </div>
      {days ? (
        <>
          <div
            class="bars"
            role="img"
            aria-label={`${total} actions over the last 14 days. Most in one day: ${Math.max(...days.map((d) => d.count))}.`}
          >
            {days.map((d) => (
              <div
                key={d.date}
                class={"cbar" + (d.today ? " today" : "") + (d.count ? "" : " none")}
                title={`${d.date}: ${d.count}`}
              >
                <div
                  class="fill"
                  style={{ height: Math.max(d.count ? 6 : 2, Math.round((d.count / max) * 100)) + "%" }}
                />
                <div class="axis">{d.today ? "Today" : d.label.slice(0, 2)}</div>
              </div>
            ))}
          </div>
          <p class="mut chartnote">
            {total
              ? `${total} actions in two weeks.`
              : "Nothing yet. Each thing the assistant does adds to this chart."}
          </p>
        </>
      ) : (
        <p class="empty">Loading.</p>
      )}
    </section>
  );
}

/** Choose the picture of the device: one of the drawings, or a photo from this computer. Kept in this browser only. */
function LookPicker(props: { value: LookId; onPick: (id: LookId) => void }): JSX.Element {
  const [open, setOpen] = useState(false);
  const [photo, setPhotoState] = useState(photoPref());
  const upload = async (ev: Event) => {
    const input = ev.currentTarget as HTMLInputElement;
    const f = input.files?.[0];
    input.value = "";
    if (!f) return;
    try {
      const url = await readPhoto(f);
      setPhoto(url);
      setPhotoState(url);
      props.onPick("photo");
    } catch (x) {
      say(x instanceof Error ? x.message : "That picture could not be used.", true);
    }
  };
  const remove = () => {
    setPhoto("");
    setPhotoState("");
    if (props.value === "photo") props.onPick("puck");
  };
  return (
    <>
      <button class="plain small" type="button" id="changelook" onClick={() => setOpen(true)}>
        Change picture
      </button>
      <Dialog id="lookdialog" title="Change the picture" open={open} onClose={() => setOpen(false)} wide>
        <p class="mut">Pick what stands for your device on Home. It is kept in this browser.</p>
        <div class="lookgrid" role="radiogroup" aria-label="What your device looks like">
          {LOOKS.map((l) => (
            <button
              key={l.id}
              type="button"
              role="radio"
              aria-checked={l.id === props.value}
              class={"lookopt" + (l.id === props.value ? " on" : "")}
              onClick={() => props.onPick(l.id)}
            >
              <span class="lookart">
                <Look id={l.id} />
              </span>
              <span class="lookname">{l.name}</span>
            </button>
          ))}
          {photo ? (
            <button
              type="button"
              role="radio"
              aria-checked={props.value === "photo"}
              class={"lookopt" + (props.value === "photo" ? " on" : "")}
              onClick={() => props.onPick("photo")}
            >
              <span class="lookart">
                <Look id="photo" />
              </span>
              <span class="lookname">Your photo</span>
            </button>
          ) : null}
        </div>
        <div class="row lookphotorow">
          <label class="plain small filebtn">
            {photo ? "Choose another photo" : "Choose a photo"}
            <input id="lookfile" type="file" class="sr" accept="image/*" onChange={(e) => void upload(e)} />
          </label>
          {photo ? (
            <button class="plain small" type="button" id="lookremove" onClick={remove}>
              Remove photo
            </button>
          ) : null}
        </div>
        <p class="mut">
          The photo is made smaller and stays in this browser; it is never sent anywhere. A picture on another web site
          cannot be used, because this page never contacts other sites. Save it to your computer, then choose the file.
        </p>
        <div class="row dlgfoot">
          <button type="button" onClick={() => setOpen(false)}>
            Done
          </button>
        </div>
      </Dialog>
    </>
  );
}

export function Dashboard(): JSX.Element {
  const s = useApp();
  const st = s.status;
  const [records, setRecords] = useState<AuditRecord[] | null>(null);
  const [update, setUpdate] = useState<UpdateStatus | null>(null);
  const [look, setLook] = useState<LookId>(lookPref());

  usePolled(async () => {
    try {
      setUpdate(await api<UpdateStatus>("GET", "/api/update"));
    } catch {
      // No updater on this device is a normal state, not a failure.
      setUpdate(null);
    }
    try {
      setRecords((await api<AuditPage>("GET", "/api/audit?limit=200")).records);
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
  const todo = attention(st, s.mode, update);
  const recent = recentItems(records ?? [], s.overview?.events ?? []);
  const u = st?.usage;
  const model = st?.llm?.model;
  const name = String(sv("device_name") || "ClawDot");
  const online = view.ring !== "off";

  return (
    <div class="home">
      <section class="card device d-status" id="statusblock">
        <div class="devart">
          <Look id={look} state={view.ring} />
        </div>
        <div class="devtext" role="status">
          <h2 id="devname">{name}</h2>
          <p class="devline">
            <span class={"sdot" + (online ? " live" : "")} aria-hidden="true" />
            <span id="statusline">{view.headline}</span>
          </p>
          {detail ? (
            <p class="mut" id="statusdetail">
              {detail}
            </p>
          ) : null}
          {model ? <p class="mut">Thinks with {model}</p> : null}
          <LookPicker
            value={look}
            onPick={(id) => {
              setLook(id);
              setLookPref(id);
            }}
          />
        </div>
      </section>

      {st?.llm && todo.length ? (
        <section class="alert d-attention" id="attention" aria-labelledby="attn-h">
          <h3 id="attn-h">
            {todo.length === 1 ? "One thing needs your attention" : `${todo.length} things need your attention`}
          </h3>
          <ul class="rows">
            {todo.map((a) => (
              <li key={a.id}>
                <div class="fix">
                  <div>
                    <strong>{a.title}</strong>
                    <div>{a.why}</div>
                  </div>
                  {a.to ? (
                    <a class="btn" href={a.to}>
                      {a.label}
                    </a>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <div class="stats">
        <Stat
          id="stat-requests"
          label="Model requests today"
          value={u ? String(u.requests_today) : "-"}
          note="to the language model"
        />
        <Stat
          id="stat-spend"
          label="Spent today"
          value={u ? "$" + u.spend_today.toFixed(2) : "-"}
          note="language model"
        />
        <Stat id="stat-heard" label="Last heard" value={heard ? ago(heard, now) : "Never"} note="a person talking" />
      </div>

      <div class="cols">
        <UsageChart days={records ? dailyCounts(records, now, zone()) : null} />
        <section class="card d-recent" id="recent">
          <div class="head">
            <h3>Recent</h3>
            <a href="#/activity">All activity</a>
          </div>
          <ul class="rows">
            {records && !recent.length ? (
              <li class="empty">
                Nothing yet. When the assistant does something, or a sender sends a message, it shows here.
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
    </div>
  );
}
