// The only place that talks to the device. Every call goes to this page's own origin (connect-src 'self').

export class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

const KEY = "clawdot-session";
let token = "";
try { token = sessionStorage.getItem(KEY) ?? ""; } catch { token = ""; }

export function hasToken(): boolean { return token !== ""; }

/** Remember the session token for this tab only (sessionStorage), or forget it with an empty string. */
export function keep(t: string | undefined): void {
  token = t ?? "";
  try {
    if (token) sessionStorage.setItem(KEY, token);
    else sessionStorage.removeItem(KEY);
  } catch { /* storage may be blocked; the token then lives in memory only */ }
}

export async function api<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (token) headers["Authorization"] = "Bearer " + token;
  const r = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  let data: { error?: string } = {};
  try { data = (await r.json()) as { error?: string }; } catch { data = {}; }
  if (!r.ok) throw new ApiError(data.error ?? `The device answered with error ${r.status}.`, r.status);
  return data as T;
}

/** The text to show for a failed call, as the page has always worded it. */
export function failText(x: unknown): { text: string; signIn: boolean } {
  const status = x instanceof ApiError ? x.status : 0;
  if (status === 401) return { text: "Please sign in again.", signIn: true };
  if (status === 429) return { text: "Too many attempts. Wait a few seconds and try again.", signIn: false };
  const msg = x instanceof Error ? x.message : "";
  return { text: msg || "Something went wrong. Try again.", signIn: false };
}
