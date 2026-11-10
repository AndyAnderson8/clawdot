import type { JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import {
  argsText,
  auditQuery,
  kb,
  NO_FILTERS,
  outcome,
  tierLabel,
  triggerLabel,
  type AuditFilters,
} from "../activity.ts";
import { Field } from "../parts.tsx";
import { fail, loadExt, useApp, zone } from "../store.ts";
import { ago, whenText } from "../time.ts";
import type { AuditPage, AuditRecord } from "../types.ts";

const STATE: Record<string, string> = {
  new: "Waiting to be read out",
  announced: "Read out",
  acknowledged: "You handled it",
  dismissed: "Dismissed",
  snoozed: "Snoozed",
};
const TIERS: [string, string][] = [
  ["", "Any"],
  ["read_only", "Only reads"],
  ["reversible", "Can be undone"],
  ["costly", "Asks you first"],
  ["destructive", "Destructive"],
];
const DECISIONS: [string, string][] = [
  ["", "Any"],
  ["ran", "Ran"],
  ["held", "Held for your yes"],
  ["approved", "Approved"],
  ["declined", "Declined"],
  ["expired", "Expired"],
  ["proposed", "Suggested"],
  ["cancelled", "Cancelled"],
  ["blocked:", "Blocked or stopped"],
];
const TRIGGERS: [string, string][] = [
  ["", "Any"],
  ["voice", "Your voice"],
  ["event", "Messages from senders"],
];

function Select(props: {
  id: string;
  label: string;
  value: string;
  options: [string, string][];
  onChange: (v: string) => void;
}): JSX.Element {
  return (
    <Field id={props.id} label={props.label}>
      <select id={props.id} value={props.value} onChange={(e) => props.onChange(e.currentTarget.value)}>
        {props.options.map(([v, t]) => (
          <option key={v} value={v}>
            {t}
          </option>
        ))}
      </select>
    </Field>
  );
}

function Row(props: { r: AuditRecord; now: number }): JSX.Element {
  const { r } = props;
  const o = outcome(r);
  const args = argsText(r.args);
  return (
    <li>
      <div class="rowhead">
        <strong>{r.tool}</strong>
        <span class={"badge" + (o.tone ? " " + o.tone : "")}>{o.label}</span>
      </div>
      <div class="mut">
        {whenText(r.ts, props.now, zone())}. {triggerLabel(r.trigger)}. {tierLabel(r.tier)}.
        {r.dry_run ? " Dry run." : ""}
      </div>
      <p>{o.text}</p>
      {args ? (
        <>
          <div class="mut">What it was asked to do (secrets are hidden)</div>
          <pre class="pre">{args}</pre>
        </>
      ) : null}
      {r.result ? (
        <>
          <div class="mut">What came back</div>
          <pre class="pre">{r.result}</pre>
        </>
      ) : null}
      {r.detail ? (
        <details>
          <summary>Show the longer result</summary>
          <pre class="pre">{r.detail}</pre>
        </details>
      ) : null}
    </li>
  );
}

export function Activity(): JSX.Element {
  const s = useApp();
  const [filters, setFilters] = useState<AuditFilters>(NO_FILTERS);
  const [rows, setRows] = useState<AuditRecord[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [meta, setMeta] = useState<AuditPage | null>(null);
  const [missing, setMissing] = useState(false);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);

  // One request at a time wins: a slow answer to an old filter must not replace a newer one.
  const load = async (f: AuditFilters, before: number | null) => {
    const mine = ++seq.current;
    setBusy(true);
    try {
      const page = await api<AuditPage>("GET", "/api/audit?" + auditQuery(f, before));
      if (mine !== seq.current) return;
      setMissing(false);
      setMeta(page);
      setNext(page.next_before);
      setRows((old) => (before === null ? page.records : [...(old ?? []), ...page.records]));
    } catch (x) {
      if (mine !== seq.current) return;
      if (x instanceof ApiError && x.status === 503) {
        setMissing(true);
        setRows([]);
      } else fail(x);
    } finally {
      if (mine === seq.current) setBusy(false);
    }
  };
  useEffect(() => {
    void load(NO_FILTERS, null);
    void loadExt();
    // Only on first show; the Refresh button and the filters load again.
  }, []);

  const apply = (f: AuditFilters) => {
    setFilters(f);
    void load(f, null);
  };
  const names = (s.ext?.tools ?? []).map((t) => t.name);
  const filtered = Object.values(filters).some((v) => v);
  const now = s.overview?.now ?? Math.floor(Date.now() / 1000);

  return (
    <div class="page">
      <header class="pagehead">
        <h2>Activity</h2>
        <p>What came in and what the assistant did, newest first. You can read these lists but not change them.</p>
      </header>
      <div class="stack">
        <section class="card">
          <h3>
            Events received<span class="count">{s.overview?.events.length ?? 0}</span>
          </h3>
          <ul class="rows" id="eventlist">
            {s.overview && !s.overview.events.length ? (
              <li class="empty">
                Nothing has arrived yet. Messages from other programs show up here once you add a sender in
                <a href="#/settings/notifications">Settings</a>.
              </li>
            ) : null}
            {s.overview?.events.map((e) => (
              <li key={e.id}>
                <div class="rowhead">
                  <strong>{e.title}</strong>
                  <span class="badge">{STATE[e.state] ?? e.state}</span>
                </div>
                <div class="mut">
                  {e.source} through the “{e.endpoint}” sender, {ago(e.received_at, s.overview?.now ?? 0)}
                </div>
                {e.text ? <div>{e.text}</div> : null}
              </li>
            ))}
          </ul>
        </section>
        <section class="card">
          <h3>
            What the assistant did<span class="count">{rows?.length ?? 0}</span>
          </h3>
          {meta ? (
            <p class="mut" id="policy">
              {meta.policy.dry_run ? "Dry run is on: actions are described, not done. " : ""}
              Limits per day: {meta.policy.max_actions_per_day} actions, {meta.policy.max_costly_per_day} of them ones
              that ask you first. <a href="#/settings">Change in Settings</a>.
            </p>
          ) : null}
          {missing ? (
            <div class="soon" role="status">
              <h3>No activity log</h3>
              <p>This device is not keeping an activity log, so there is nothing to show.</p>
            </div>
          ) : (
            <>
              <details id="auditfilterbox" open={filtered}>
                <summary>Filter the list{filtered ? <span class="badge good">on</span> : null}</summary>
                <form
                  class="panel flat"
                  id="auditfilter"
                  onSubmit={(ev) => {
                    ev.preventDefault();
                    apply(filters);
                  }}
                >
                  <Field id="a-tool" label="Tool" hint="The exact tool name, for example set_timer.">
                    <input
                      id="a-tool"
                      type="text"
                      list="a-tools"
                      autoCapitalize="none"
                      autoComplete="off"
                      value={filters.tool}
                      onInput={(e) => setFilters({ ...filters, tool: e.currentTarget.value })}
                    />
                    <datalist id="a-tools">
                      {names.map((n) => (
                        <option key={n} value={n} />
                      ))}
                    </datalist>
                  </Field>
                  <Select
                    id="a-tier"
                    label="How careful it is"
                    value={filters.tier}
                    options={TIERS}
                    onChange={(v) => apply({ ...filters, tier: v })}
                  />
                  <Select
                    id="a-decision"
                    label="What happened"
                    value={filters.decision}
                    options={DECISIONS}
                    onChange={(v) => apply({ ...filters, decision: v })}
                  />
                  <Select
                    id="a-trigger"
                    label="What started it"
                    value={filters.trigger}
                    options={TRIGGERS}
                    onChange={(v) => apply({ ...filters, trigger: v })}
                  />
                  <div class="row">
                    <button>Show</button>
                    <button class="plain" type="button" onClick={() => apply(NO_FILTERS)} disabled={!filtered}>
                      Clear filters
                    </button>
                    <button class="plain" type="button" onClick={() => void load(filters, null)}>
                      Refresh
                    </button>
                  </div>
                </form>
              </details>
              {rows && !rows.length ? (
                <p class="empty" id="auditempty">
                  {filtered
                    ? "Nothing matches these filters."
                    : "Nothing yet. When the assistant sets a timer, calls a tool or asks for your yes, it shows up here."}
                </p>
              ) : null}
              <ul class="rows" id="auditlist" aria-busy={busy ? "true" : "false"}>
                {rows?.map((r) => (
                  <Row key={r.id} r={r} now={now} />
                ))}
              </ul>
              {next !== null ? (
                <div class="row">
                  <button
                    class="plain"
                    type="button"
                    id="auditmore"
                    disabled={busy}
                    onClick={() => void load(filters, next)}
                  >
                    Show older
                  </button>
                </div>
              ) : null}
              {meta && (meta.log.corrupt_lines > 0 || meta.log.write_errors > 0) ? (
                <p class="mut" id="loghealth">
                  {meta.log.corrupt_lines > 0 ? meta.log.corrupt_lines + " lines in the log could not be read. " : ""}
                  {meta.log.write_errors > 0
                    ? meta.log.write_errors + " actions could not be written to the log. Check the disk."
                    : ""}
                </p>
              ) : null}
              {meta ? (
                <p class="mut">
                  The log keeps about {kb(meta.log.max_file_bytes * 2)} of history and drops the oldest first.
                </p>
              ) : null}
            </>
          )}
        </section>
      </div>
    </div>
  );
}
