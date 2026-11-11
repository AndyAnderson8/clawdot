// The first-run setup and the sign-in page. They share one look: a dark panel in the colour of the app's rail, with the
// speaker's light ring drawn large, and an open, roomy form beside it.
import type { ComponentChildren, JSX } from "preact";
import { useState } from "preact/hooks";
import { api, keep } from "./api.ts";
import { Logo, Wordmark } from "./brand.tsx";
import { Forgot } from "./forgot.tsx";
import { fail, say, start, useApp } from "./store.ts";

const LEDS = 12;

/** The ring of twelve lights on top of the speaker, lit one after another once when the page opens. */
function BigRing(): JSX.Element {
  const dots = [];
  for (let i = 0; i < LEDS; i++) {
    const a = (i / LEDS) * Math.PI * 2 - Math.PI / 2;
    dots.push(
      <circle
        key={i}
        class="led"
        style={{ "--i": String(i) }}
        cx={120 + 90 * Math.cos(a)}
        cy={120 + 90 * Math.sin(a)}
        r={5}
      />,
    );
  }
  return (
    <div class="bigring" aria-hidden="true">
      <svg viewBox="0 0 240 240">
        <circle class="disc" cx="120" cy="120" r="112" />
        <circle class="inner" cx="120" cy="120" r="66" />
        {dots}
      </svg>
      <Logo />
    </div>
  );
}

/** The page around both forms: the brand panel, then the form and the footer. */
export function AuthShell(props: { children: ComponentChildren }): JSX.Element {
  const s = useApp();
  return (
    <div class="auth">
      <aside class="authbrand" id="hero">
        <div class="authlogo">
          <Wordmark />
        </div>
        <BigRing />
        <div class="authtext">
          <h1>Your voice assistant, on your own speaker.</h1>
          <p>You choose the model, the tools and the network it lives on.</p>
        </div>
      </aside>
      <div class="authmain">
        <div class="authcard">{props.children}</div>
        <footer id="foot" class="authfoot">
          <span class="fmark">
            <i aria-hidden="true" />
            <span id="ver">{"ClawDot " + s.version}</span>
          </span>
        </footer>
      </div>
    </div>
  );
}

/** A password box with a button to show what was typed. */
function PasswordField(props: {
  id: string;
  label: string;
  value: string;
  onInput: (v: string) => void;
  autoComplete: string;
  placeholder?: string;
}): JSX.Element {
  const [shown, setShown] = useState(false);
  const common = {
    id: props.id,
    autoComplete: props.autoComplete,
    placeholder: props.placeholder,
    maxLength: 128,
    value: props.value,
    onInput: (e: Event) => props.onInput((e.currentTarget as HTMLInputElement).value),
  };
  return (
    <div class="authfield">
      <label for={props.id}>{props.label}</label>
      <div class="pwbox">
        {shown ? <input type="text" {...common} /> : <input type="password" {...common} />}
        <button
          class="plain small showpw"
          type="button"
          aria-pressed={shown}
          aria-controls={props.id}
          onClick={() => setShown(!shown)}
        >
          {shown ? "Hide" : "Show"}
        </button>
      </div>
    </div>
  );
}

/** What to tell the person about the two boxes while they type. */
function pairHint(a: string, b: string): string {
  if (a.length < 8) return "Use 8 or more characters.";
  if (!b) return "Type it again to be sure.";
  return a === b ? "They match." : "They do not match yet.";
}

export function Setup(): JSX.Element {
  const s = useApp();
  const [pw1, setPw1] = useState("");
  const [pw2, setPw2] = useState("");
  const finish = async (body: { mode: string; password?: string }) => {
    try {
      const r = await api<{ token?: string }>("POST", "/api/setup", body);
      keep(r.token);
      say("");
      await start();
    } catch (x) {
      fail(x);
    }
  };
  const setPassword = (ev: Event) => {
    ev.preventDefault();
    if (pw1 !== pw2) {
      say("The two passwords differ. Type the same one twice.", true);
      return;
    }
    if (pw1.length < 8) {
      say("Use at least 8 characters.", true);
      return;
    }
    const password = pw1;
    setPw1("");
    setPw2("");
    void finish({ mode: "password", password });
  };
  return (
    <section id="setup">
      <h2>Set up ClawDot</h2>
      <p class="authlead">
        Choose a password for the admin page. Until you do, anyone on your private network who opens this page could set
        one instead.
      </p>
      <form id="pwform" onSubmit={setPassword}>
        <PasswordField
          id="pw1"
          label="Password"
          autoComplete="new-password"
          value={pw1}
          onInput={setPw1}
          placeholder="8 or more characters"
        />
        <PasswordField id="pw2" label="Repeat the password" autoComplete="new-password" value={pw2} onInput={setPw2} />
        <p class="authhint" role="status" id="pwhint">
          {pw1 || pw2 ? pairHint(pw1, pw2) : "Webhooks have their own tokens; this password is only for this page."}
        </p>
        <button class="authgo">Create password</button>
      </form>
      <form
        id="openform"
        class="authskip"
        onSubmit={(ev) => {
          ev.preventDefault();
          void finish({ mode: "open" });
        }}
      >
        <p class="mut">
          Prefer no password? You can leave it open on this network, but anyone here could then use this page and see
          what your webhooks receive.
        </p>
        <button class="plain small" id="openbtn" disabled={!s.openAllowed || !s.peerPrivate}>
          Leave it open
        </button>
      </form>
    </section>
  );
}

export function Login(): JSX.Element {
  const [pw, setPw] = useState("");
  const submit = async (ev: Event) => {
    ev.preventDefault();
    const password = pw;
    setPw("");
    try {
      const r = await api<{ token: string }>("POST", "/api/login", { password });
      keep(r.token);
      say("");
      await start();
    } catch (x) {
      const status = (x as { status?: number }).status;
      say(
        status === 401
          ? "That password was not accepted. Check it and try again."
          : status === 429
            ? "Too many attempts. Wait a few seconds and try again."
            : (x as Error).message,
        true,
      );
    }
  };
  return (
    <section id="login">
      <h2>Sign in</h2>
      <p class="authlead">Welcome back. Enter the admin password to manage your speaker.</p>
      <form id="loginform" onSubmit={submit}>
        <PasswordField id="pw" label="Password" autoComplete="current-password" value={pw} onInput={setPw} />
        <button class="authgo">Sign in</button>
      </form>
      <Forgot />
    </section>
  );
}
