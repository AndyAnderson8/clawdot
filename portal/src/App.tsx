import type { ComponentType, JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { Activity } from "./tabs/Activity.tsx";
import { Dashboard } from "./tabs/Dashboard.tsx";
import { Clock } from "./tabs/Reminders.tsx";
import { Media } from "./tabs/Media.tsx";
import { SettingsPage } from "./tabs/SettingsPage.tsx";
import { AuthShell, Login, Setup } from "./auth.tsx";
import { Logo, Wordmark } from "./brand.tsx";
import { useApp } from "./store.ts";

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
  { id: "media", label: "Media", view: Media },
  { id: "settings", label: "Settings", view: SettingsPage },
];
/** Reachable from Home (Recent, the chart) but not in the menu. */
const HIDDEN: Tab[] = [{ id: "activity", label: "Activity", view: Activity }];

/** The destination and sub-page named in the address fragment (`#/settings/assistant`), or the first. */
export function routeFromHash(hash: string): { tab: Tab; sub: string } {
  const [name0 = "", sub0 = ""] = hash.replace(/^#\/?/, "").split("/");
  // Features used to be a destination of its own.
  const [name, sub] =
    name0 === "features" ? ["settings", "features"] : name0 === "bluetooth" ? ["media", ""] : [name0, sub0];
  return { tab: [...TABS, ...HIDDEN].find((t) => t.id === name) ?? TABS[0], sub };
}

const ICONS: Record<string, string> = {
  dashboard: "M3 11.5 12 4l9 7.500V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1Z",
  clock: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 5v4.500l3 2",
  media: "M9 18V5l12-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0Zm12-2a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z",
  features:
    "M12 3v4m0 10v4M3 12h4m10 0h4M5.600 5.600l2.800 2.800m7.200 7.200 2.800 2.800m0-12.800-2.800 2.800m-7.200 7.200-2.800 2.800",
  settings:
    "M4 7h9m4 0h3M4 17h3m4 0h9M15 4.500a2.500 2.500 0 1 0 0 5 2.500 2.500 0 0 0 0-5ZM9 14.500a2.500 2.500 0 1 0 0 5 2.500 2.500 0 0 0 0-5Z",
};

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
  if (s.view === "setup" || s.view === "login") {
    return (
      <>
        <AuthShell>{s.view === "setup" ? <Setup /> : <Login />}</AuthShell>
        <div id="msg" class={"toast" + (s.toast.bad ? " err" : "")} role="status">
          {s.toast.text || null}
        </div>
      </>
    );
  }
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
