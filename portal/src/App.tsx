import type { ComponentType, JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api, keep } from "./api.ts";
import { Activity } from "./tabs/Activity.tsx";
import { Dashboard } from "./tabs/Dashboard.tsx";
import { Clock } from "./tabs/Reminders.tsx";
import { Bluetooth } from "./tabs/Bluetooth.tsx";
import { SettingsPage } from "./tabs/SettingsPage.tsx";
import { Forgot } from "./forgot.tsx";
import { fail, say, start, useApp } from "./store.ts";

/** What a view is given: the part of the address after the destination (`#/settings/assistant` gives "assistant"). */
interface ViewProps {
  sub: string;
}
interface Tab {
  id: string;
  label: string;
  view: ComponentType<ViewProps>;
}
/** The destinations, Home first. Everything you can change, including the Features switches, is under Settings. */
export const TABS: [Tab, ...Tab[]] = [
  { id: "dashboard", label: "Home", view: Dashboard },
  { id: "clock", label: "Clock", view: Clock },
  { id: "bluetooth", label: "Bluetooth", view: Bluetooth },
  { id: "settings", label: "Settings", view: SettingsPage },
];
/** Reachable from Home (Recent, the chart) but not in the menu. */
const HIDDEN: Tab[] = [{ id: "activity", label: "Activity", view: Activity }];

/** The destination and sub-page named in the address fragment (`#/settings/assistant`), or the first. */
export function routeFromHash(hash: string): { tab: Tab; sub: string } {
  const [name0 = "", sub0 = ""] = hash.replace(/^#\/?/, "").split("/");
  // Features used to be a destination of its own.
  const [name, sub] = name0 === "features" ? ["settings", "features"] : [name0, sub0];
  return { tab: [...TABS, ...HIDDEN].find((t) => t.id === name) ?? TABS[0], sub };
}

const Logo = () => (
  <svg class="logo" aria-hidden="true">
    <use href="#mk" />
  </svg>
);

const ICONS: Record<string, string> = {
  dashboard: "M3 11.5 12 4l9 7.500V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1Z",
  clock: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 5v4.500l3 2",
  bluetooth: "m7 7 10 10-5 5V2l5 5L7 17",
  features:
    "M12 3v4m0 10v4M3 12h4m10 0h4M5.600 5.600l2.800 2.800m7.200 7.200 2.800 2.800m0-12.800-2.800 2.800m-7.200 7.200-2.800 2.800",
  settings:
    "M4 7h9m4 0h3M4 17h3m4 0h9M15 4.500a2.500 2.500 0 1 0 0 5 2.500 2.500 0 0 0 0-5ZM9 14.500a2.500 2.500 0 1 0 0 5 2.500 2.500 0 0 0 0-5Z",
};

/** The name, with the light ring of the speaker standing in for the o. Read out as plain "ClawDot". */
function Wordmark() {
  return (
    <span class="wordmark" aria-hidden="true">
      ClawD
      <span class="ring" />t
    </span>
  );
}

function Rail(props: { tab: Tab }) {
  return (
    <div class="rail" id="band">
      <a class="brand" href="#/" id="home" aria-label="ClawDot, go to Home">
        <span class="chip">
          <Logo />
        </span>
        <Wordmark />
      </a>
      <nav class="nav" id="tabs" aria-label="Sections">
        {TABS.map((t) => (
          <a key={t.id} href={"#/" + t.id} id={"tab-" + t.id} aria-current={t.id === props.tab.id ? "page" : undefined}>
            <svg class="ico" viewBox="0 0 24 24" aria-hidden="true">
              <path d={ICONS[t.id] ?? ""} />
            </svg>
            <span>{t.label}</span>
          </a>
        ))}
      </nav>
    </div>
  );
}

function Setup() {
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
      <p class="mut">
        Tokens for senders are separate and always required. Until you finish, anyone on your private network who opens
        this page could do it instead, so do it now.
      </p>
      <form id="pwform" class="panel" onSubmit={setPassword}>
        <h3>Protect with an admin password</h3>
        <div class="row">
          <input
            id="pw1"
            type="password"
            autoComplete="new-password"
            placeholder="Password (8 or more characters)"
            aria-label="New password"
            maxLength={128}
            value={pw1}
            onInput={(e) => setPw1(e.currentTarget.value)}
          />
        </div>
        <div class="row">
          <input
            id="pw2"
            type="password"
            autoComplete="new-password"
            placeholder="Repeat the password"
            aria-label="Repeat the password"
            maxLength={128}
            value={pw2}
            onInput={(e) => setPw2(e.currentTarget.value)}
          />
        </div>
        <div class="row">
          <button>Set password</button>
        </div>
      </form>
      <form
        id="openform"
        class="panel quiet"
        onSubmit={(ev) => {
          ev.preventDefault();
          void finish({ mode: "open" });
        }}
      >
        <h3>Or leave it open on this network</h3>
        <p class="mut">Anyone on this network can use this page and see what your senders send.</p>
        <div class="row">
          <button class="danger" id="openbtn" disabled={!s.openAllowed || !s.peerPrivate}>
            Leave open
          </button>
        </div>
      </form>
    </section>
  );
}

function Login() {
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
      <form class="panel" id="loginform" onSubmit={submit}>
        <div class="row">
          <input
            id="pw"
            type="password"
            autoComplete="current-password"
            placeholder="Password"
            aria-label="Password"
            value={pw}
            onInput={(e) => setPw(e.currentTarget.value)}
          />
          <button>Sign in</button>
        </div>
      </form>
      <Forgot />
    </section>
  );
}

export function App(): JSX.Element {
  const s = useApp();
  const [hash, setHash] = useState(location.hash);
  useEffect(() => {
    const f = () => setHash(location.hash);
    window.addEventListener("hashchange", f);
    return () => window.removeEventListener("hashchange", f);
  }, []);
  const { tab, sub } = routeFromHash(hash);
  const inApp = s.view === "app";
  const first = useRef(true);

  // Keep the title in step with the page, and move focus to the page content after a move, so a screen reader
  // starts reading the new page (not on the first load).
  useEffect(() => {
    if (!inApp) return;
    document.title = tab.label + " - ClawDot";
    if (first.current) first.current = false;
    else document.getElementById("panel")?.focus({ preventScroll: true });
  }, [inApp, tab.id, sub, tab.label]);

  const Active = tab.view;
  return (
    <div class={"shell" + (inApp ? " in" : "")}>
      {inApp ? <Rail tab={tab} /> : null}
      <div class="stage">
        <div id="top">
          {s.mode === "open" ? (
            <div id="banner" role="alert">
              Unprotected (open mode): anyone on this network can use this page.{" "}
              <a
                href="#/settings/security"
                id="setpw"
                onClick={() => setTimeout(() => document.getElementById("newpw")?.focus(), 50)}
              >
                Set a password
              </a>
            </div>
          ) : null}
        </div>
        <main>
          {s.view === "setup" || s.view === "login" ? (
            <div id="hero">
              <Logo />
              <div>
                <h1>ClawDot</h1>
                <p class="sub">A voice assistant that runs on your own speaker.</p>
              </div>
            </div>
          ) : null}
          {s.view === "setup" ? <Setup /> : null}
          {s.view === "login" ? <Login /> : null}
          {inApp ? (
            <div id="app">
              <div id="panel" tabIndex={-1}>
                <Active key={tab.id} sub={sub} />
              </div>
            </div>
          ) : null}
          <footer id="foot">
            <span class="fmark">
              <i aria-hidden="true" />
              <span id="ver">{"ClawDot " + s.version}</span>
            </span>
            {s.mode ? (
              <a
                href="#/settings/about"
                id="newlink"
                onClick={() =>
                  setTimeout(() => {
                    const d = document.getElementById("about") as HTMLDetailsElement | null;
                    if (d) {
                      d.open = true;
                      d.scrollIntoView();
                    }
                  }, 50)
                }
              >
                What is new in this version
              </a>
            ) : null}
          </footer>
        </main>
      </div>
      <div id="msg" class={"toast" + (s.toast.bad ? " err" : "")} role="status">
        {s.toast.text || null}
      </div>
    </div>
  );
}
