import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { parseSettingsFile, settingsChanges, type SettingChange } from "../forms.ts";
import { saveFile } from "../parts.tsx";
import { Updates } from "./Updates.tsx";
import { adopt, fail, get, say, useApp } from "../store.ts";
import type { About as AboutData, Settings } from "../types.ts";
import { ask } from "../confirm.tsx";

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
  const [picked, setPicked] = useState<{ settings: Record<string, unknown>; changes: SettingChange[] } | null>(null);
  const [importErr, setImportErr] = useState("");

  useEffect(() => {
    api<AboutData>("GET", "/api/about").then(setAbout).catch(fail);
  }, []);

  const exportSettings = async () => {
    try {
      const doc = await api<unknown>("GET", "/api/settings/export");
      saveFile("clawdot-settings.json", JSON.stringify(doc, null, 2));
      say("Saved your settings. The file holds no passwords or keys.");
    } catch (x) {
      fail(x);
    }
  };
  const resetAll = async () => {
    const saved = Object.entries(get().settings?.fields ?? {}).filter(([, f]) => f.saved);
    if (!saved.length) {
      say("Nothing to reset: every setting is already at its default.");
      return;
    }
    const sure = await ask(
      "Put all " +
        saved.length +
        " saved settings back to their defaults? Your webhooks, skills, memory and password are kept.",
      "Reset them",
    );
    if (!sure) return;
    try {
      const next = await api<Settings>("PUT", "/api/settings", Object.fromEntries(saved.map(([k]) => [k, null])));
      adopt(next, true);
      say("All settings are back to their defaults.");
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
      adopt(next, true);
      setPicked(null);
      say("Settings imported.");
    } catch (x) {
      if (x instanceof ApiError && x.status === 400) setImportErr(x.message);
      else fail(x);
    }
  };

  return (
    <div class="stack">
      <section class="card">
        <h3>About ClawDot</h3>
        <p>
          ClawDot is a voice assistant that runs on a small device you own, such as a smart speaker, a Raspberry Pi or a
          computer. You choose the language model and the tools it may use. ClawDot listens, thinks, acts and speaks.
        </p>
        <dl class="kv">
          <dt>Version</dt>
          <dd id="aboutver">{s.version}</dd>
          {about?.build ? (
            <>
              <dt>Build</dt>
              <dd id="buildinfo">{buildText(about)}</dd>
            </>
          ) : null}
          {s.settings?.speakers_listen ? (
            <>
              <dt>Speakers connect on</dt>
              <dd>{s.settings.speakers_listen}</dd>
            </>
          ) : null}
        </dl>
        <details id="about">
          <summary>Release notes</summary>
          {about ? <pre id="notes">{about.changelog}</pre> : null}
        </details>
      </section>

      <Updates />

      <section class="card">
        <h3>Troubleshooting</h3>
        <p class="mut">
          Warnings and errors, what the assistant did, and a diagnostics file to send with a bug report are on the Logs
          page.
        </p>
        <div class="row">
          <a class="btn" id="tologs" href="#/logs/problems">
            Open the logs
          </a>
        </div>
      </section>

      <section class="card sgroup">
        <h3>Back up and restore settings</h3>
        <p class="mut">
          Keep a copy of your settings as a file. Restore it later, or load it on another ClawDot to copy your setup.
          Passwords and keys are never in the file; a key is only the name of the variable that holds it.
        </p>
        <div class="srow">
          <div class="slabel">
            <label for="exportdl">Back up</label>
            <p class="mut">Downloads your settings as a small file.</p>
          </div>
          <div class="sctl">
            <button class="plain" type="button" id="exportdl" onClick={exportSettings}>
              Download backup
            </button>
          </div>
        </div>
        <form
          class="srow"
          id="importform"
          onSubmit={(ev) => {
            ev.preventDefault();
            void apply();
          }}
        >
          <div class="slabel">
            <label for="importfile">Restore from a backup</label>
            <p class="mut">Choose a backup file. You see what would change before anything does.</p>
          </div>
          <div class="sctl">
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
                  <button>Restore these settings</button>
                </>
              ) : (
                <p id="importsame">Everything in this file is already how it is here.</p>
              )
            ) : null}
            <p class="errtext" role="alert">
              {importErr}
            </p>
          </div>
        </form>
      </section>
      <section class="card sgroup">
        <h3>Start over</h3>
        <div class="srow">
          <div class="slabel">
            <label for="resetall">Reset all settings</label>
            <p class="mut">
              Puts every setting you saved here back to its default. Your webhooks, skills, memory and password are
              kept.
            </p>
          </div>
          <div class="sctl">
            <button class="danger" type="button" id="resetall" onClick={resetAll}>
              Reset all settings
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
