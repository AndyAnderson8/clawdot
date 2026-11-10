// A forgotten password on a speaker with no shell: the way back in (REQ-CFG-071..073), and the box that appears while a reset window is open.
import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, keep } from "./api.ts";
import { say, start, useApp } from "./store.ts";

const GUIDE = "https://github.com/AndyAnderson8/clawdot/blob/dev/docs/guides/forgot-password.md";

/** On the sign-in page: how to get back in, and the form that appears while the speaker's reset window is open. */
export function Forgot(): JSX.Element {
  const s = useApp();
  const [pw, setPw] = useState("");

  const send = async (ev: Event) => {
    ev.preventDefault();
    const password = pw;
    setPw("");
    try {
      const r = await api<{ token: string }>("POST", "/api/recover/window", { password });
      keep(r.token);
      say("");
      await start();
    } catch (x) {
      say(x instanceof Error ? x.message : "Something went wrong. Try again.", true);
    }
  };

  return (
    <>
      <details class="help" id="forgot">
        <summary>Forgot your password?</summary>
        <p id="forgothelp">
          A forgotten password is reset on the speaker itself, with its USB cable or at its console.{" "}
          <a href={GUIDE} target="_blank" rel="noopener noreferrer">
            The guide
          </a>{" "}
          has the steps for your device.
        </p>
      </details>
      {s.windowSecs > 0 ? (
        <form class="panel" id="windowform" onSubmit={send}>
          <h3>The speaker is ready for a new password</h3>
          <p class="mut">Someone held the action button, so you can set a new password for the next few minutes.</p>
          <div class="row">
            <input
              id="wpw"
              type="password"
              autoComplete="new-password"
              placeholder="New password (8 or more characters)"
              aria-label="New password"
              maxLength={128}
              value={pw}
              onInput={(e) => setPw(e.currentTarget.value)}
            />
            <button>Set password</button>
          </div>
        </form>
      ) : null}
    </>
  );
}
