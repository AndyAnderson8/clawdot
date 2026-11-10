// What the page knows, and the actions that change it. One small store; components subscribe with useApp().
import { useEffect, useRef, useState } from "preact/hooks";
import { api, ApiError, failText, hasToken, keep } from "./api.ts";
import type { AuthState, Extensions, Overview, Settings, Status } from "./types.ts";

export type View = "" | "setup" | "login" | "app";

export interface AppState {
  view: View;
  mode: string;
  version: string;
  legacy: boolean;
  openAllowed: boolean;
  peerPrivate: boolean;
  /** A recovery code exists (REQ-CFG-067). */
  recovery: boolean;
  /** Seconds the local reset window has left. */
  windowSecs: number;
  /** A recovery code just made, shown once and then dropped. */
  recoveryCode: string;
  settings: Settings | null;
  /** Bumped when settings are replaced, so forms start again from the new values. */
  settingsRev: number;
  status: Status | null;
  overview: Overview | null;
  ext: Extensions | null;
  /** Webhook tokens just made or rotated, kept in memory only until the page closes. */
  fresh: Record<string, string>;
  spoke: boolean;
  /** Bumped to make every open tab fetch its own data again. */
  tick: number;
  toast: { text: string; bad: boolean };
}

let state: AppState = {
  view: "",
  mode: "",
  version: "",
  legacy: false,
  openAllowed: false,
  peerPrivate: false,
  recovery: false,
  windowSecs: 0,
  recoveryCode: "",
  settings: null,
  settingsRev: 0,
  status: null,
  overview: null,
  ext: null,
  fresh: {},
  spoke: false,
  tick: 0,
  toast: { text: "", bad: false },
};
const subscribers = new Set<() => void>();

export function get(): AppState {
  return state;
}

export function set(patch: Partial<AppState>): void {
  state = { ...state, ...patch };
  subscribers.forEach((f) => f());
}

/** The current state; the component renders again when it changes. */
export function useApp(): AppState {
  const [, force] = useState(0);
  const rendered = useRef(state);
  rendered.current = state;
  useEffect(() => {
    const f = () => force((n) => n + 1);
    subscribers.add(f);
    // Effects run after paint. If the state changed between rendering and subscribing (a fast local
    // answer), nobody told us, so catch up now.
    if (rendered.current !== state) f();
    return () => {
      subscribers.delete(f);
    };
  }, []);
  return state;
}

/** Run `fn` when the component appears and again on every refresh (every 10 seconds, or after a change). */
export function usePolled(fn: () => void | Promise<void>): void {
  const tick = useApp().tick;
  useEffect(() => {
    void fn();
    // `fn` is a new closure on every render; only a refresh (the tick) should run it again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick]);
}

// ----- small helpers the components share -----

export function sv(key: string): string | number | boolean | null {
  const f = state.settings?.fields[key];
  return f ? f.value : null;
}

/** The device's time zone. */
export function zone(): string {
  return String(sv("zone") || "UTC");
}

let toastTimer = 0;
export function say(text: string, bad = false): void {
  set({ toast: { text, bad } });
  clearTimeout(toastTimer);
  if (text && !bad) toastTimer = window.setTimeout(() => set({ toast: { text: "", bad: false } }), 5000);
}

/** Show a failed call. A 401 sends the person back to sign in. */
export function fail(x: unknown): void {
  const f = failText(x);
  if (f.signIn) {
    keep("");
    void start();
  }
  say(f.text, true);
}

// ----- loading -----

export async function loadCore(): Promise<void> {
  const st = await api<{ status: Status }>("GET", "/api/status");
  const ov = await api<Overview>("GET", "/api/overview");
  set({ status: st.status, overview: ov });
}

/** Refresh the shared data, then tell the open tab to refresh its own. */
export async function load(): Promise<void> {
  try {
    await loadCore();
    set({ tick: state.tick + 1 });
  } catch (x) {
    fail(x);
  }
}

/** Tools, Connections, and Skills share one listing. */
export async function loadExt(): Promise<void> {
  try {
    set({ ext: await api<Extensions>("GET", "/api/tools") });
  } catch (x) {
    fail(x);
  }
}

/** Send a change; the device answers with the new listing, which replaces the old one. */
export async function change(method: string, path: string, body: unknown, okText?: string): Promise<boolean> {
  try {
    set({ ext: await api<Extensions>(method, path, body) });
    if (okText) say(okText);
    return true;
  } catch (x) {
    fail(x);
    return false;
  }
}

/** Decide what to show: setup, sign-in, or the portal. */
export async function start(): Promise<void> {
  let st: AuthState;
  try {
    st = await api<AuthState>("GET", "/api/auth");
  } catch (x) {
    say(x instanceof Error ? x.message : "Something went wrong. Try again.", true);
    return;
  }
  set({
    mode: st.mode,
    version: st.version,
    legacy: !!st.legacy,
    openAllowed: !!st.open_allowed,
    peerPrivate: !!st.peer_private,
    recovery: !!st.recovery,
    windowSecs: st.reset_window_secs ?? 0,
  });
  if (st.mode === "setup") {
    set({ view: "setup" });
    if (!st.peer_private) say("Setup can only be finished from a device on your own network.", true);
    return;
  }
  if (st.mode === "password" && !hasToken()) {
    set({ view: "login" });
    return;
  }
  try {
    const settings = await api<Settings>("GET", "/api/settings");
    set({ settings, settingsRev: state.settingsRev + 1 });
    await loadCore();
    set({ view: "app" });
  } catch (x) {
    if (x instanceof ApiError && x.status === 401) {
      keep("");
      set({ view: "login" });
      say("Please sign in.", true);
    } else fail(x);
  }
}

export function signedOut(): void {
  set({ fresh: {} });
}
