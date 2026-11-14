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
  /** Seconds the local reset window has left. */
  windowSecs: number;
  settings: Settings | null;
  /** Bumped when settings are replaced, so forms start again from the new values. */
  settingsRev: number;
  /** Goes up when the provider's key is saved or removed, so lists that need the key are fetched again. */
  keyRev: number;
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
  windowSecs: 0,
  settings: null,
  settingsRev: 0,
  keyRev: 0,
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
  const latest = useRef(fn);
  latest.current = fn;
  const running = useRef(false);
  const again = useRef(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    // One load at a time: a refresh that arrives while the last is still out runs once when it has finished,
    // so a slow device is not asked again and again.
    const go = async () => {
      if (running.current) {
        again.current = true;
        return;
      }
      running.current = true;
      try {
        await latest.current();
      } finally {
        running.current = false;
      }
      if (again.current && alive.current) {
        again.current = false;
        void go();
      }
    };
    void go();
  }, [tick]);
}

/**
 * A counter for loads that can overlap. `const current = next()` starts one; `current()` says whether it is still the
 * newest. Call `next()` again when a change comes back with the new data, so a slower load that started earlier
 * cannot put the old data back.
 */
export function useSeq(): () => () => boolean {
  const n = useRef(0);
  return () => {
    const mine = ++n.current;
    return () => mine === n.current;
  };
}

/** Run a call; a failure is shown to the person and `undefined` comes back. */
export async function attempt<T>(job: () => Promise<T>): Promise<T | undefined> {
  try {
    return await job();
  } catch (x) {
    fail(x);
    return undefined;
  }
}

// ----- small helpers the components share -----

export function sv(key: string): string | number | boolean | null {
  const f = state.settings?.fields[key];
  return f ? f.value : null;
}

/** The device's clock, in seconds: what it last reported, or this browser's clock before it has. */
export function deviceNow(): number {
  return state.overview?.now ?? Math.floor(Date.now() / 1000);
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
    signedOut();
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

let extSeq = 0;
let extLoading: Promise<void> | null = null;

/** Tools, Connections, and Skills share one listing, fetched once however many parts ask for it at the same time. */
export function loadExt(): Promise<void> {
  if (extLoading) return extLoading;
  const mine = ++extSeq;
  const job = (async () => {
    try {
      const ext = await api<Extensions>("GET", "/api/tools");
      // A change that came back meanwhile has newer data than this answer.
      if (mine === extSeq) set({ ext });
    } catch (x) {
      fail(x);
    } finally {
      extLoading = null;
    }
  })();
  extLoading = job;
  return job;
}

/** Send a change; the device answers with the new listing, which replaces the old one. */
export async function change(method: string, path: string, body: unknown, okText?: string): Promise<boolean> {
  try {
    const ext = await api<Extensions>(method, path, body);
    extSeq++;
    set({ ext });
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
    say(failText(x).text, true);
    return;
  }
  set({
    mode: st.mode,
    version: st.version,
    legacy: !!st.legacy,
    openAllowed: !!st.open_allowed,
    peerPrivate: !!st.peer_private,
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
      signedOut();
      set({ view: "login" });
      say("Please sign in.", true);
    } else fail(x);
  }
}

/**
 * Take the device's answer to a settings change into the stored settings. `remount` also bumps `settingsRev`, so
 * every form starts again from the new values; leave it off when the form that was saved starts itself again, so the
 * other forms keep what the person has typed and not yet saved.
 */
export function adopt(next: Settings, remount = false): Settings {
  const merged = Object.assign({}, state.settings, next);
  set(remount ? { settings: merged, settingsRev: state.settingsRev + 1 } : { settings: merged });
  return merged;
}

/** Forget everything the last session held (webhook tokens, settings, listings), so the next sign-in starts clean. */
export function signedOut(): void {
  set({ fresh: {}, settings: null, ext: null, overview: null, status: null });
}
