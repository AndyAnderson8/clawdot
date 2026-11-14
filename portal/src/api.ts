// The only place that talks to the device. Every call goes to this page's own origin (connect-src 'self').

export class ApiError extends Error {
  status: number;
  /** A short machine-readable cause, when the device gives one (for example `no_speaker`). */
  reason: string;
  constructor(message: string, status: number, reason = "") {
    super(message);
    this.status = status;
    this.reason = reason;
  }
}

const KEY = "clawdot-session";
let token = "";
try {
  token = sessionStorage.getItem(KEY) ?? "";
} catch {
  token = "";
}

export function hasToken(): boolean {
  return token !== "";
}

/** Remember the session token for this tab only (sessionStorage), or forget it with an empty string. */
export function keep(t: string | undefined): void {
  token = t ?? "";
  try {
    if (token) sessionStorage.setItem(KEY, token);
    else sessionStorage.removeItem(KEY);
  } catch {
    /* storage may be blocked; the token then lives in memory only */
  }
}

/** A name placed in a request path, so a slash, a space or a question mark in it cannot change the path. */
export function enc(name: string | number): string {
  return encodeURIComponent(String(name));
}

/** How long a call may take before the page gives up on it, in milliseconds. */
export const TIMEOUT_MS = 30000;

export async function api<T = unknown>(
  method: string,
  path: string,
  body?: unknown,
  timeoutMs = TIMEOUT_MS,
): Promise<T> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (token) headers["Authorization"] = "Bearer " + token;
  const stop = new AbortController();
  const timer = setTimeout(() => stop.abort(), timeoutMs);
  let r: Response;
  let data: { error?: string; message?: string; reason?: string } = {};
  try {
    r = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: stop.signal,
    });
    try {
      data = (await r.json()) as { error?: string; message?: string; reason?: string };
    } catch {
      if (stop.signal.aborted) throw new Error("aborted");
      data = {};
    }
  } catch (x) {
    // A device that does not answer must not leave a button waiting for ever.
    if (stop.signal.aborted) throw new Error("The device did not answer in time. Try again.");
    throw x;
  } finally {
    clearTimeout(timer);
  }
  if (!r.ok)
    throw new ApiError(
      data.message ?? data.error ?? `The device answered with error ${r.status}.`,
      r.status,
      data.reason ?? "",
    );
  return data as T;
}

/** The text to show for a failed call, as the page has always worded it. */
export function failText(x: unknown): { text: string; signIn: boolean } {
  const status = x instanceof ApiError ? x.status : 0;
  if (status === 401) return { text: "Please sign in again.", signIn: true };
  if (status === 429) return { text: "Too many attempts. Wait a few seconds and try again.", signIn: false };
  // A browser words "the device cannot be reached" in its own way ("Failed to fetch", "Load failed"); say it plainly.
  if (x instanceof TypeError)
    return { text: "Cannot reach the device. Check that it is on and on your network, then try again.", signIn: false };
  const msg = x instanceof Error ? x.message : "";
  return { text: msg || "Something went wrong. Try again.", signIn: false };
}
