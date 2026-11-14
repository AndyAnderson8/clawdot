import type { JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { attempt, fail } from "../store.ts";
import { ago } from "../time.ts";
import type { UpdateStatus } from "../types.ts";
import { ask } from "../confirm.tsx";

/** What each step of an install is called while the person waits. */
function phaseText(u: UpdateStatus): string {
  switch (u.phase) {
    case "downloading":
      return "Downloading the update.";
    case "verifying":
      return "Checking that it is genuine.";
    case "installing":
      return "Installing.";
    case "installed":
      return u.supervised
        ? "Installed. ClawDot is restarting."
        : "Installed. Restart ClawDot to start the new version.";
    case "restarting":
      return "ClawDot is restarting. This page reloads when it is back.";
    default:
      return "";
  }
}

const WORKING = ["downloading", "verifying", "installing", "installed", "restarting"];

/** Whether a newer version exists, what is new in it, and a way to install it after a clear confirmation. */
export function Updates(): JSX.Element | null {
  const [u, setU] = useState<UpdateStatus | null>(null);
  const [missing, setMissing] = useState(false);
  const [checking, setChecking] = useState(false);
  const before = useRef<string | null>(null);

  const load = async () => {
    try {
      setU(await api<UpdateStatus>("GET", "/api/update"));
    } catch (x) {
      // A refused answer is shown; no answer at all means the device is restarting, so keep waiting.
      if (x instanceof ApiError) {
        if (x.status === 503 || x.status === 404) setMissing(true);
        else fail(x);
      }
    }
  };
  useEffect(() => {
    void load();
  }, []);

  // While an install runs, watch it; after a restart the page comes back to a new version.
  const working = !!u && WORKING.includes(u.phase);
  // Installed on a device nothing restarts: there is nothing more to watch for until a person restarts it.
  const watching = !!u && working && !(u.phase === "installed" && !u.supervised);
  useEffect(() => {
    if (!watching) return;
    const t = setInterval(() => void load(), 2000);
    return () => clearInterval(t);
  }, [watching]);
  useEffect(() => {
    // After an install and a restart, this page is talking to the new version: start over with it.
    if (before.current && u && u.current !== before.current) location.reload();
  }, [u]);

  const check = async () => {
    setChecking(true);
    await attempt(async () => {
      setU(await api<UpdateStatus>("POST", "/api/update/check"));
    });
    setChecking(false);
  };
  const install = async () => {
    const v = u?.latest?.version;
    if (!v) return;
    const restart = u.supervised
      ? "ClawDot will restart by itself."
      : "You will need to restart ClawDot yourself afterwards.";
    const sure = await ask(
      `Install version ${v}? ClawDot downloads it, checks that the project signed it, and replaces the version you have. ${restart} The old version is kept.`,
      "Install",
    );
    if (!sure) return;
    before.current = u.current;
    await attempt(async () => {
      setU(await api<UpdateStatus>("POST", "/api/update/apply", { version: v }));
    });
  };

  if (missing) return null;
  const now = Math.floor(Date.now() / 1000);
  const latest = u?.latest;
  return (
    <section class="card" id="updates">
      <div class="head">
        <h3>Updates</h3>
        <button class="plain small" type="button" id="checknow" disabled={checking || working} onClick={check}>
          {checking ? "Checking" : "Check now"}
        </button>
      </div>
      {u ? (
        <>
          <dl class="kv">
            <dt>Last checked</dt>
            <dd id="lastchecked">{u.last_checked ? ago(u.last_checked, now) : "Not yet"}</dd>
          </dl>
          {u.error ? (
            <p class="errtext" id="updateerr">
              {u.error}
            </p>
          ) : null}
          {u.available && latest ? (
            <>
              <p id="updateavail">
                <strong>Version {latest.version} is available.</strong>
              </p>
              {latest.notes ? (
                <details id="updatenotes" class="help">
                  <summary>What is new</summary>
                  <pre class="pre">{latest.notes}</pre>
                </details>
              ) : null}
              {latest.page.startsWith("https://") ? (
                <p class="mut">
                  <a href={latest.page} target="_blank" rel="noopener noreferrer">
                    Release page
                  </a>
                </p>
              ) : null}
              {!u.can_apply && u.cannot_apply_because ? (
                <p class="mut" id="cannotupdate">
                  {u.cannot_apply_because}
                </p>
              ) : null}
              <div class="row">
                <button type="button" id="installupdate" disabled={!u.can_apply || working} onClick={install}>
                  Install version {latest.version}
                </button>
              </div>
            </>
          ) : u.last_checked && !u.error ? (
            <p id="uptodate">You are up to date.</p>
          ) : null}
          {phaseText(u) ? (
            <p role="status" id="updatephase">
              {phaseText(u)}
            </p>
          ) : null}
          {u.apply_error ? (
            <p class="errtext" role="alert" id="applyerr">
              {u.apply_error}
            </p>
          ) : null}
          <p class="mut">
            {u.enabled
              ? `ClawDot looks about once a day at ${u.source || "the release page"}. Turn that off under `
              : "Checking every day is off. Turn it on under "}
            <a href="#/settings/features">Features</a>. Nothing about you or your speaker is sent.
          </p>
        </>
      ) : (
        <p class="empty">Loading.</p>
      )}
    </section>
  );
}
