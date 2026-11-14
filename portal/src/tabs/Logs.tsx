import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api } from "../api.ts";
import { saveFile } from "../parts.tsx";
import { attempt, say, usePolled, zone } from "../store.ts";
import { fmt } from "../time.ts";
import type { LogList } from "../types.ts";
import { Activity } from "./Activity.tsx";

/** What each page of Logs is, in the order of its tabs. */
const PAGES: { id: string; label: string; intro: string }[] = [
  { id: "activity", label: "Activity", intro: "What came in and what the assistant did, newest first." },
  {
    id: "problems",
    label: "Problems",
    intro: "Warnings and errors from ClawDot itself, and a file to send with a bug report.",
  },
  { id: "conversations", label: "Conversations", intro: "What was said to the assistant and what it answered." },
];

const LEVELS: [string, string][] = [
  ["", "All problems"],
  ["error", "Errors"],
  ["warn", "Warnings"],
];

/** ClawDot's own warnings and errors, searchable, and the diagnostics file. */
function Problems(): JSX.Element {
  const [logs, setLogs] = useState<LogList | null>(null);
  const [level, setLevel] = useState("");
  const [query, setQuery] = useState("");
  const load = async () => {
    await attempt(async () => {
      setLogs(await api<LogList>("GET", "/api/logs?limit=200"));
    });
  };
  usePolled(load);
  const diagnostics = async () => {
    await attempt(async () => {
      const doc = await api<unknown>("GET", "/api/diagnostics");
      saveFile("clawdot-diagnostics.json", JSON.stringify(doc, null, 2));
      say("Saved the diagnostics file. It holds no passwords, keys, or tokens.");
    });
  };
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const shown = (logs?.lines ?? []).filter(
    (l) => (!level || l.level === level) && words.every((w) => l.text.toLowerCase().includes(w)),
  );
  return (
    <div class="stack">
      <section class="card" id="problemcard">
        <div class="head">
          <h3>
            Problems<span class="count">{logs?.lines.length ?? 0}</span>
          </h3>
          <button class="plain small" type="button" id="diagdl" onClick={() => void diagnostics()}>
            Download diagnostics
          </button>
        </div>
        <p class="mut">
          What ClawDot has warned about or failed at since it started, newest first. If something is not working,
          download the diagnostics file and send it with your bug report: it has the version, your settings without any
          password or key, the names of your webhooks, how many tools and records there are, and these problems. It
          never has audio, anything you said, what it remembers, or a webhook token.
        </p>
        <div class="toolbar">
          <input
            id="logsearch"
            type="search"
            placeholder="Search problems"
            aria-label="Search problems"
            value={query}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />
          <select id="loglevel" aria-label="Show" value={level} onChange={(e) => setLevel(e.currentTarget.value)}>
            {LEVELS.map(([v, t]) => (
              <option key={v} value={v}>
                {t}
              </option>
            ))}
          </select>
          <button class="plain small" type="button" onClick={() => void load()}>
            Refresh
          </button>
        </div>
        {logs && !shown.length ? (
          <p class="empty" id="lognone">
            {logs.lines.length ? "No problem matches that." : "No problems since ClawDot started."}
          </p>
        ) : (
          <ul class="rows loglist" id="loglines">
            {shown.map((l, i) => (
              <li key={l.ts + "-" + i} class="logrow">
                <span class={"badge " + (l.level === "error" ? "bad" : "")}>
                  {l.level === "error" ? "Error" : "Warning"}
                </span>
                <div>
                  <div class="pre">{l.text}</div>
                  <div class="mut">{fmt(l.ts, zone(), { dateStyle: "short", timeStyle: "medium" })}</div>
                </div>
              </li>
            ))}
          </ul>
        )}
        <p class="mut">
          Only ClawDot's own messages are listed, kept in memory since it started. The newest {logs?.max_lines ?? 200}{" "}
          are kept, and a restart clears them. This page refreshes itself every few seconds.
        </p>
      </section>
    </div>
  );
}

/** Conversations are not kept; this says so and what a future option would be. */
function Conversations(): JSX.Element {
  return (
    <div class="stack">
      <section class="card" id="convcard">
        <h3>Conversations</h3>
        <div class="blank">
          <span class="fico" aria-hidden="true">
            <svg class="ico" viewBox="0 0 24 24">
              <path d="M4 5h16v11H9l-5 4Z" />
            </svg>
          </span>
          <h3>Not recorded</h3>
          <p class="mut">
            ClawDot does not keep what people say to it or what it answers, because that is private. Activity shows what
            the assistant did. An option to keep conversation text here, off unless you turn it on and with a way to
            delete it, is planned.
          </p>
          <a class="btn" href="#/logs/activity">
            See what it did
          </a>
        </div>
      </section>
    </div>
  );
}

/** Everything that says what happened: what the assistant did, what went wrong, and (later) what was said. */
export function Logs(props: { sub: string }): JSX.Element {
  const page = PAGES.find((p) => p.id === props.sub) ?? PAGES[0] ?? { id: "activity", label: "Activity", intro: "" };
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Logs</h2>
        <p>{page.intro}</p>
      </header>
      <nav class="tabs" aria-label="Logs sections" id="logtabs">
        {PAGES.map((p) => (
          <a key={p.id} href={"#/logs/" + p.id} aria-current={p.id === page.id ? "page" : undefined}>
            {p.label}
          </a>
        ))}
      </nav>
      {page.id === "activity" ? <Activity embedded /> : null}
      {page.id === "problems" ? <Problems /> : null}
      {page.id === "conversations" ? <Conversations /> : null}
    </div>
  );
}
