import type { ComponentType, JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api, keep } from "./api.ts";
import { Activity } from "./tabs/Activity.tsx";
import { Dashboard } from "./tabs/Dashboard.tsx";
import { Reminders } from "./tabs/Reminders.tsx";
import { SettingsPage } from "./tabs/SettingsPage.tsx";
import { fail, say, start, sv, useApp } from "./store.ts";

/** What a view is given: the part of the address after the destination (`#/settings/assistant` gives "assistant"). */
interface ViewProps {
  sub: string;
}
interface Tab {
  id: string;
  label: string;
  view: ComponentType<ViewProps>;
}
/** The four destinations, Dashboard first. Everything that is configuration lives under Settings. */
export const TABS: [Tab, ...Tab[]] = [
  { id: "dashboard", label: "Dashboard", view: Dashboard },
  { id: "reminders", label: "Reminders", view: Reminders },
  { id: "activity", label: "Activity", view: Activity },
  { id: "settings", label: "Settings", view: SettingsPage },
];

/** The destination and sub-page named in the address fragment (`#/settings/assistant`), or the first. */
export function routeFromHash(hash: string): { tab: Tab; sub: string } {
  const [name = "", sub = ""] = hash.replace(/^#\/?/, "").split("/");
  return { tab: TABS.find((t) => t.id === name) ?? TABS[0], sub };
}

const Logo = () => (
  <svg class="logo" aria-hidden="true">
    <use href="#mk" />
  </svg>
);

function Bar(props: { tab: Tab }) {
  return (
    <div class="bar" id="band">
      <div class="brand">
        <Logo />
        <h1 id="devname">{String(sv("device_name") || "ClawDot")}</h1>
      </div>
      <nav class="nav" id="tabs" aria-label="Sections">
        {TABS.map((t) => (
          <a key={t.id} href={"#/" + t.id} id={"tab-" + t.id} aria-current={t.id === props.tab.id ? "page" : undefined}>
            {t.label}
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
        Tokens for inputs are separate and always required. Until you finish, anyone on your private network who opens
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
        <p class="mut">Anyone on this network can use this page and see what your inputs send.</p>
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
      <p class="mut">
        Forgot it? On the device, run <code>clawdot --reset-admin</code> and open this page again.
      </p>
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
    <>
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
        {inApp ? <Bar tab={tab} /> : null}
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
          <span id="ver">{"ClawDot " + s.version}</span>{" "}
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
      <div id="msg" class={"toast" + (s.toast.bad ? " err" : "")} role="status">
        {s.toast.text || null}
      </div>
    </>
  );
}
