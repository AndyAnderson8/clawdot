import type { ComponentType, JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api, keep } from "./api.ts";
import { Activity } from "./tabs/Activity.tsx";
import { Agents } from "./tabs/Agents.tsx";
import { Connections } from "./tabs/Connections.tsx";
import { Home } from "./tabs/Home.tsx";
import { Notes } from "./tabs/Notes.tsx";
import { Reminders } from "./tabs/Reminders.tsx";
import { Settings } from "./tabs/Settings.tsx";
import { Skills } from "./tabs/Skills.tsx";
import { Tools } from "./tabs/Tools.tsx";
import { Webhooks } from "./tabs/Webhooks.tsx";
import { Forgot, RecoveryNotice, showCode } from "./recovery.tsx";
import { fail, say, start, sv, useApp } from "./store.ts";

interface Tab {
  id: string;
  label: string;
  view: ComponentType;
}
export const TABS: [Tab, ...Tab[]] = [
  { id: "home", label: "Home", view: Home },
  { id: "reminders", label: "Reminders", view: Reminders },
  { id: "notes", label: "Notes", view: Notes },
  { id: "agents", label: "Agents", view: Agents },
  { id: "activity", label: "Activity", view: Activity },
  { id: "tools", label: "Tools", view: Tools },
  { id: "connections", label: "Connections", view: Connections },
  { id: "skills", label: "Skills", view: Skills },
  { id: "webhooks", label: "Webhooks", view: Webhooks },
  { id: "settings", label: "Settings", view: Settings },
];

/** The tab named in the address fragment (`#/reminders`), or the first. */
export function tabFromHash(hash: string): Tab {
  const name = hash.replace(/^#\/?/, "").split("/")[0];
  return TABS.find((t) => t.id === name) ?? TABS[0];
}

const Logo = () => (
  <svg class="logo" aria-hidden="true">
    <use href="#mk" />
  </svg>
);

function Band(props: { tab: Tab }) {
  const s = useApp();
  const st = s.status ?? {};
  const ok = !!(st.llm && st.clock_valid && st.llm.configured);
  const cls = !st.llm ? "wait" : ok ? "ok" : "warn";
  const text = !st.llm ? "Starting" : ok ? "Ready" : !st.clock_valid ? "Clock not set" : "Needs setup";

  const onKey = (ev: KeyboardEvent) => {
    const i = TABS.findIndex((t) => t.id === props.tab.id);
    const n =
      ev.key === "ArrowRight"
        ? i + 1
        : ev.key === "ArrowLeft"
          ? i - 1
          : ev.key === "Home"
            ? 0
            : ev.key === "End"
              ? TABS.length - 1
              : null;
    if (n === null) return;
    ev.preventDefault();
    const next = TABS[(n + TABS.length) % TABS.length] ?? TABS[0];
    location.hash = "#/" + next.id;
    document.getElementById("tab-" + next.id)?.focus();
  };

  return (
    <div class="band" id="band">
      <div class="who">
        <Logo />
        <h1 id="devname">{String(sv("device_name") || "ClawDot")}</h1>
        <span class={"state " + cls} id="state">
          <svg viewBox="0 0 28 16" aria-hidden="true">
            <ellipse cx="14" cy="8" rx="11.5" ry="5" fill="none" stroke="currentColor" stroke-width="2.8" />
          </svg>
          <span id="statetext">{text}</span>
        </span>
      </div>
      <nav class="tabs" id="tabs" role="tablist" aria-label="Sections" onKeyDown={onKey}>
        {TABS.map((t) => {
          const on = t.id === props.tab.id;
          return (
            <a
              key={t.id}
              href={"#/" + t.id}
              role="tab"
              id={"tab-" + t.id}
              aria-controls="panel"
              aria-selected={on ? "true" : "false"}
              tabIndex={on ? 0 : -1}
            >
              {t.label}
            </a>
          );
        })}
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
      const r = await api<{ token?: string; recovery_code?: string }>("POST", "/api/setup", body);
      keep(r.token);
      showCode(r.recovery_code);
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
        Webhook tokens for agents are separate and always required. Until you finish, anyone on your private network who
        opens this page could do it instead, so do it now.
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
        <p class="warn">Anyone on this network can use this page and see what your agents send.</p>
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
  const tab = tabFromHash(hash);
  const inApp = s.view === "app";

  // Keep the selected tab visible in the scrolling strip, and the title in step with it.
  useEffect(() => {
    if (!inApp) return;
    document.title = tab.label + " - ClawDot";
    document.getElementById("tab-" + tab.id)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [inApp, tab.id, tab.label]);

  const Active = tab.view;
  return (
    <>
      <div id="top">
        {s.mode === "open" ? (
          <div id="banner" role="alert">
            Unprotected (open mode): anyone on this network can use this page.{" "}
            <a
              href="#/settings"
              id="setpw"
              onClick={() => setTimeout(() => document.getElementById("newpw")?.focus(), 50)}
            >
              Set a password
            </a>
          </div>
        ) : null}
        {inApp ? <Band tab={tab} /> : null}
      </div>
      <main>
        {s.recoveryCode ? <RecoveryNotice /> : null}
        {!s.recoveryCode && (s.view === "setup" || s.view === "login") ? (
          <div id="hero">
            <Logo />
            <div>
              <h1>
                Claw<span>Dot</span>
              </h1>
              <p class="sub">A voice assistant that runs on your own speaker.</p>
            </div>
          </div>
        ) : null}
        {!s.recoveryCode && s.view === "setup" ? <Setup /> : null}
        {!s.recoveryCode && s.view === "login" ? <Login /> : null}
        {inApp && !s.recoveryCode ? (
          <div id="app">
            <div id="panel" role="tabpanel" tabIndex={-1} aria-labelledby={"tab-" + tab.id}>
              <Active key={tab.id} />
            </div>
          </div>
        ) : null}
        <footer id="foot">
          <span id="ver">{"ClawDot " + s.version}</span>
          {s.mode ? (
            <a
              href="#/settings"
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
