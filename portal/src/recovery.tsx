// Forgotten password: the one-time recovery code, the "forgot it" form, and the Settings control (REQ-CFG-067..072).
import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, keep } from "./api.ts";
import { CopyButton } from "./parts.tsx";
import { fail, say, set, start, useApp } from "./store.ts";

const GUIDE = "https://github.com/AndyAnderson8/clawdot/blob/dev/docs/guides/forgot-password.md";

/** Show a recovery code the device just made, and keep it in memory only. */
export function showCode(code: string | undefined): void {
  if (code) set({ recoveryCode: code });
}

/** Shown once after a password is set or reset. The person must say they saved it before going on. */
export function RecoveryNotice(): JSX.Element {
  const s = useApp();
  const [saved, setSaved] = useState(false);
  return (
    <section id="recoverynote">
      <h2>Save your recovery code</h2>
      <p class="mut">
        If you ever forget your password, this code lets you set a new one. It is shown only this once. Write it down or
        keep it in a password manager, somewhere other than this device.
      </p>
      <div class="panel">
        <p>
          <code id="rcode">{s.recoveryCode}</code>
        </p>
        <div class="row">
          <CopyButton text={s.recoveryCode} label="Copy the code" />
        </div>
        <div class="row">
          <label>
            <input id="rsaved" type="checkbox" checked={saved} onChange={(e) => setSaved(e.currentTarget.checked)} /> I
            have saved this code
          </label>
        </div>
        <div class="row">
          <button id="rcont" disabled={!saved} onClick={() => set({ recoveryCode: "" })}>
            Continue
          </button>
        </div>
      </div>
    </section>
  );
}

/** On the sign-in page: help text, and the forms for a recovery code and for an open reset window. */
export function Forgot(): JSX.Element {
  const s = useApp();
  const [open, setOpen] = useState(false);
  const [code, setCode] = useState("");
  const [pw, setPw] = useState("");
  const [wpw, setWpw] = useState("");

  const send = async (path: string, body: object) => {
    try {
      const r = await api<{ token: string; recovery_code?: string }>("POST", path, body);
      keep(r.token);
      showCode(r.recovery_code);
      say("");
      await start();
    } catch (x) {
      say(x instanceof Error ? x.message : "Something went wrong. Try again.", true);
    }
  };
  const recover = (ev: Event) => {
    ev.preventDefault();
    const body = { code, password: pw };
    setPw("");
    void send("/api/recover", body);
  };
  const window_ = (ev: Event) => {
    ev.preventDefault();
    const body = { password: wpw };
    setWpw("");
    void send("/api/recover/window", body);
  };

  return (
    <>
      <p class="mut" id="forgothelp">
        Forgot it? Use your recovery code. No code? Hold the action button for 10 seconds, or see{" "}
        <a href={GUIDE} target="_blank" rel="noopener noreferrer">
          the guide
        </a>
        .
      </p>
      {s.windowSecs > 0 ? (
        <form class="panel" id="windowform" onSubmit={window_}>
          <h3>The speaker is ready for a new password</h3>
          <p class="mut">
            Someone held the action button, so you can set a new password here for the next few minutes without a code.
          </p>
          <div class="row">
            <input
              id="wpw"
              type="password"
              autoComplete="new-password"
              placeholder="New password (8 or more characters)"
              aria-label="New password"
              maxLength={128}
              value={wpw}
              onInput={(e) => setWpw(e.currentTarget.value)}
            />
            <button>Set password</button>
          </div>
        </form>
      ) : null}
      {s.recovery ? (
        <div class="row">
          <button class="plain" type="button" id="forgot" aria-expanded={open} onClick={() => setOpen(!open)}>
            Forgot your password?
          </button>
        </div>
      ) : null}
      {s.recovery && open ? (
        <form class="panel" id="recoverform" onSubmit={recover}>
          <h3>Use your recovery code</h3>
          <div class="row">
            <input
              id="rcin"
              type="text"
              autoComplete="off"
              autoCapitalize="characters"
              spellcheck={false}
              placeholder="Recovery code"
              aria-label="Recovery code"
              maxLength={40}
              value={code}
              onInput={(e) => setCode(e.currentTarget.value)}
            />
          </div>
          <div class="row">
            <input
              id="rcpw"
              type="password"
              autoComplete="new-password"
              placeholder="New password (8 or more characters)"
              aria-label="New password"
              maxLength={128}
              value={pw}
              onInput={(e) => setPw(e.currentTarget.value)}
            />
            <button>Set new password</button>
          </div>
        </form>
      ) : null}
    </>
  );
}

/** In Settings: whether a recovery code exists, and a button to make a new one. */
export function RecoverySettings(): JSX.Element {
  const s = useApp();
  const make = async () => {
    try {
      const r = await api<{ recovery_code: string }>("POST", "/api/admin/recovery");
      showCode(r.recovery_code);
      set({ recovery: true });
    } catch (x) {
      fail(x);
    }
  };
  if (s.mode !== "password") return <></>;
  return (
    <div class="panel" id="recsec">
      <h3>Recovery code</h3>
      <p class="mut">
        {s.recovery
          ? "A recovery code is set. Making a new one stops the old one working."
          : "No recovery code yet. Make one so you can get back in if you forget your password."}
      </p>
      <div class="row">
        <button type="button" id="recmake" onClick={make}>
          {s.recovery ? "Make a new recovery code" : "Make a recovery code"}
        </button>
      </div>
    </div>
  );
}
