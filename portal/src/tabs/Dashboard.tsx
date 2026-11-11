import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { attention, requestDays, lastHeard, recentItems, speakerView, type Day } from "../dashboard.ts";
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

/** Bars for the last two weeks: how many requests the assistant made to the language model each day. */
function UsageChart(props: { days: Day[] | null }): JSX.Element {
  const days = props.days;
  const max = Math.max(1, ...(days ?? []).map((d) => d.count));
  const total = (days ?? []).reduce((n, d) => n + d.count, 0);
  return (
    <section class="card" id="usage" aria-labelledby="usage-h">
      <div class="head">
        <h3 id="usage-h">Requests, last 14 days</h3>
        <a href="#/activity">All activity</a>
      </div>
      {days ? (
        <>
          <div
            class="bars"
            role="img"
            aria-label={`${total} requests over the last 14 days. Most in one day: ${Math.max(...days.map((d) => d.count))}.`}
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
              ? `${total} requests in two weeks.`
              : "Nothing yet. Each question the assistant answers adds to this chart."}
          </p>
        </>
      ) : (
        <p class="empty">Loading.</p>
      )}
    </section>
  );
}

/** Choose the picture of the device: one of the drawings. Your own picture is a side option below them. Kept in this browser only. */
function LookPicker(props: { value: LookId; onPick: (id: LookId) => void }): JSX.Element {
  const [open, setOpen] = useState(false);
  const [photo, setPhotoState] = useState(photoPref());
  const [address, setAddress] = useState("");
  const keep = (url: string) => {
    setPhoto(url);
    setPhotoState(url);
    props.onPick(url ? "photo" : "puck");
  };
  const upload = async (ev: Event) => {
    const input = ev.currentTarget as HTMLInputElement;
    const f = input.files?.[0];
    input.value = "";
    if (!f) return;
    try {
      keep(await readPhoto(f));
    } catch (x) {
      say(x instanceof Error ? x.message : "That picture could not be used.", true);
    }
  };
  // An address is tried first, so one that is not a picture is said so instead of leaving a blank space on Home.
  const useAddress = (ev: Event) => {
    ev.preventDefault();
    const url = address.trim();
    if (!/^https?:\/\/\S+$/.test(url)) {
      say("Enter the whole address of the picture, starting with https://.", true);
      return;
    }
    const probe = new Image();
    probe.onload = () => {
      keep(url);
      setAddress("");
    };
    probe.onerror = () => say("That address did not load as a picture.", true);
    probe.src = url;
  };
  return (
    <>
      <button class="plain small" type="button" id="changelook" onClick={() => setOpen(true)}>
        Change picture
      </button>
      <Dialog id="lookdialog" title="Change the picture" open={open} onClose={() => setOpen(false)}>
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
              <span class="lookname">Your picture</span>
            </button>
          ) : null}
        </div>
        <details class="ownpic" id="ownpic" open={Boolean(photo)}>
          <summary>Use your own picture instead</summary>
          <div class="ownbody">
            <div class="row">
              <label class="plain small filebtn">
                Upload a photo
                <input id="lookfile" type="file" class="sr" accept="image/*" onChange={(e) => void upload(e)} />
              </label>
              {photo ? (
                <button class="plain small" type="button" id="lookremove" onClick={() => keep("")}>
                  Remove it
                </button>
              ) : null}
            </div>
            <form class="row addressrow" onSubmit={useAddress}>
              <input
                id="lookurl"
                type="text"
                inputMode="url"
                autoCapitalize="none"
                autoComplete="off"
                placeholder="Or paste a picture address, https://..."
                aria-label="Picture address"
                value={address}
                onInput={(e) => setAddress(e.currentTarget.value)}
              />
              <button class="plain small" id="lookurlgo">
                Use address
              </button>
            </form>
            <p class="mut">
              An uploaded photo is made smaller and kept in this browser. With an address, this browser loads the
              picture from that site each time Home opens. Either way, the choice stays in this browser.
            </p>
          </div>
        </details>
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
        <UsageChart days={u?.day !== undefined ? requestDays(u.history ?? [], u.day) : null} />
        <section class="card d-recent" id="recent">
          <div class="head">
            <h3>Recent</h3>
            <a href="#/activity">All activity</a>
          </div>
          <ul class="rows">
            {records && !recent.length ? (
              <li class="empty">
                Nothing yet. When the assistant does something, or a webhook sends a message, it shows here.
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
