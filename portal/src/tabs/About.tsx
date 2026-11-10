import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { parseSettingsFile, settingsChanges, type SettingChange } from "../forms.ts";
import { saveFile } from "../parts.tsx";
import { fail, get, say, set, useApp, zone } from "../store.ts";
import { fmt } from "../time.ts";
import type { About as AboutData, LogList, Settings } from "../types.ts";

function buildText(a: AboutData): string {
  const b = a.build;
  if (!b) return "";
  const features = b.features.length ? " Built with: " + b.features.join(", ") + "." : "";
  return `A ${b.profile} build for ${b.os} on ${b.arch}.${features}`;
}

/** Version, release notes, what to send when something is wrong, and moving settings between devices. */
export function About(): JSX.Element {
  const s = useApp();
  const [about, setAbout] = useState<AboutData | null>(null);
  const [logs, setLogs] = useState<LogList | null>(null);
  const [picked, setPicked] = useState<{ settings: Record<string, unknown>; changes: SettingChange[] } | null>(null);
  const [importErr, setImportErr] = useState("");

  useEffect(() => {
    api<AboutData>("GET", "/api/about").then(setAbout).catch(fail);
  }, []);

  const loadLogs = async () => {
    try {
      setLogs(await api<LogList>("GET", "/api/logs?limit=100"));
    } catch (x) {
      fail(x);
    }
  };
  const diagnostics = async () => {
    try {
      const doc = await api<unknown>("GET", "/api/diagnostics");
      saveFile("clawdot-diagnostics.json", JSON.stringify(doc, null, 2));
      say("Saved the diagnostics file. It holds no passwords, keys, or tokens.");
    } catch (x) {
      fail(x);
    }
  };
  const exportSettings = async () => {
    try {
      const doc = await api<unknown>("GET", "/api/settings/export");
      saveFile("clawdot-settings.json", JSON.stringify(doc, null, 2));
      say("Saved your settings. The file holds no passwords or keys.");
    } catch (x) {
      fail(x);
    }
  };
  const choose = async (ev: Event) => {
    const input = ev.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    setPicked(null);
    setImportErr("");
    if (!file) return;
    if (file.size > 16 * 1024) {
      setImportErr("That file is too big to be a ClawDot settings file.");
      return;
    }
    const parsed = parseSettingsFile(await file.text());
    if (typeof parsed === "string") {
      setImportErr(parsed);
      return;
    }
    const current = get().settings;
    setPicked({ settings: parsed.settings, changes: current ? settingsChanges(current, parsed.settings) : [] });
  };
  const apply = async () => {
    if (!picked) return;
    setImportErr("");
    try {
      const next = await api<Settings>("PUT", "/api/settings", picked.settings);
      set({ settings: Object.assign({}, get().settings, next), settingsRev: get().settingsRev + 1 });
      setPicked(null);
      say("Settings imported.");
    } catch (x) {
      if (x instanceof ApiError && x.status === 400) setImportErr(x.message);
      else fail(x);
    }
  };

  return (
    <>
      <h3>About</h3>
      <p id="aboutver">
        ClawDot {s.version}. It runs entirely on your own device; this page never contacts anything outside your
        network.
      </p>
      {about?.build ? (
        <p class="mut" id="buildinfo">
          {buildText(about)}
        </p>
      ) : null}
      <details id="about">
        <summary>Release notes</summary>
        {about ? <pre id="notes">{about.changelog}</pre> : null}
      </details>

      <h3>When something is wrong</h3>
      <p>
        Download a diagnostics file and send it with your bug report. It has the version, your settings without any
        password or key, the names of your inputs, how many tools and records there are, and the latest problems. It
        never has audio, anything you said, your notes, or an input token.
      </p>
      <div class="row">
        <button type="button" id="diagdl" onClick={diagnostics}>
          Download diagnostics
        </button>
      </div>
      <details
        id="logs"
        onToggle={(ev) => {
          if (ev.currentTarget.open && !logs) void loadLogs();
        }}
      >
        <summary>Recent problems</summary>
        {logs ? (
          logs.lines.length ? (
            <pre id="loglines" class="pre">
              {logs.lines
                .map(
                  (l) =>
                    fmt(l.ts, zone(), { dateStyle: "short", timeStyle: "medium" }) + "  " + l.level + "  " + l.text,
                )
                .join("\n")}
            </pre>
          ) : (
            <p id="lognone">No problems since ClawDot started.</p>
          )
        ) : null}
        <div class="row">
          <button class="plain" type="button" onClick={loadLogs}>
            Refresh
          </button>
        </div>
        <p class="mut">
          Only ClawDot's own messages are listed, from memory since it started. The newest {logs?.max_lines ?? 200} are
          kept.
        </p>
      </details>

      <h3>Move settings to another device</h3>
      <p>
        Save your settings as a file, and load that file on another ClawDot. Passwords and keys are never in the file; a
        key is only the name of the variable that holds it.
      </p>
      <div class="row">
        <button class="plain" type="button" id="exportdl" onClick={exportSettings}>
          Save my settings
        </button>
      </div>
      <form
        class="panel"
        id="importform"
        onSubmit={(ev) => {
          ev.preventDefault();
          void apply();
        }}
      >
        <h3>Load a settings file</h3>
        <label for="importfile">Settings file</label>
        <input id="importfile" type="file" accept="application/json,.json" onChange={choose} />
        {picked ? (
          picked.changes.length ? (
            <>
              <p>Loading this file would change:</p>
              <ul class="rows" id="importlist">
                {picked.changes.map((c) => (
                  <li key={c.key}>
                    <strong>{c.label}</strong>
                    <div class="mut">
                      {c.from} to {c.to}
                    </div>
                  </li>
                ))}
              </ul>
              <button>Load these settings</button>
            </>
          ) : (
            <p id="importsame">Everything in this file is already how it is here.</p>
          )
        ) : null}
        <p class="errtext" role="alert">
          {importErr}
        </p>
      </form>
    </>
  );
}
