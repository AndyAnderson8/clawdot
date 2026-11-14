// The shared harness of the browser smoke test: the one browser and the one mock device, the helpers every flow
// uses (open, eq, T, step, ...), and the final report. Flows live in ./areas and receive this as a Ctx.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { Browser, sleep } from "./chrome.ts";
import { startMock, type Mock } from "./mock-api.ts";
import { HELPERS } from "./page-helpers.ts";

export interface Ctx {
  b: Browser;
  mock: Mock;
  /** The directory for screenshots (SHOTS), if any. */
  shots: string | undefined;
  open: (mode: "setup" | "password" | "open", hash?: string, extra?: Record<string, unknown>) => Promise<void>;
  T: (expr: string) => Promise<unknown>;
  eq: (expr: string, want: unknown, what: string) => Promise<void>;
  setTime: (id: string, hhmm: string) => Promise<void>;
  section: (id: string, label: string) => Promise<void>;
  viewport: (width: number) => Promise<unknown>;
  shot: (name: string, o?: { dark?: boolean; width?: number }) => Promise<void>;
  step: (name: string, fn: () => void | Promise<void>) => Promise<void>;
  /** Close the browser and the mock, print every result and the summary line, and return the exit code. */
  report: () => Promise<number>;
}

export async function createContext(chrome: string): Promise<Ctx> {
  const results: { name: string; ok: boolean; why?: string }[] = [];
  const mock: Mock = await startMock("open");
  const b = await Browser.launch(chrome);
  const shots = process.env["SHOTS"];
  if (shots) mkdirSync(shots, { recursive: true });

  async function step(name: string, fn: () => void | Promise<void>): Promise<void> {
    try {
      await fn();
      results.push({ name, ok: true });
    } catch (e) {
      let page = "";
      try {
        page = await b.ev<string>(
          "location.href + ' | ' + document.body.innerText.replace(/\\s+/g, ' ').slice(0, 160)",
        );
      } catch {
        /* page gone */
      }
      results.push({
        name,
        ok: false,
        why:
          (e as Error).message +
          (page ? "\n       page: " + page : "") +
          (b.problems.length ? "\n       problems: " + b.problems.join("; ") : ""),
      });
    }
  }

  async function open(mode: "setup" | "password" | "open", hash = "", extra: Record<string, unknown> = {}) {
    // Start from a clean browser session on the device's origin, then a clean device.
    await b.goto(mock.url + "/api/auth");
    await b.ev("sessionStorage.clear(); localStorage.clear()");
    mock.reset(mode);
    for (const [k, v] of Object.entries(extra)) {
      const have = mock.state[k];
      mock.state[k] =
        v && typeof v === "object" && !Array.isArray(v) && have && typeof have === "object" && !Array.isArray(have)
          ? { ...have, ...v }
          : v;
    }
    await b.goto(mock.url + "/" + hash);
    await b.ev(HELPERS);
  }
  const T = (expr: string) => b.ev(`T.${expr}`);
  const eq = async (expr: string, want: unknown, what: string) => assert.deepEqual(await b.ev(expr), want, what);
  /** Type a time (24-hour "HH:MM") into the hour, minute and AM or PM boxes of a time field. */
  const setTime = async (id: string, hhmm: string) => {
    const [h = 0, m = 0] = hhmm.split(":").map(Number);
    const twelve = await b.ev<boolean>(`!!T.q('#${id}-p')`);
    await T(`set('#${id}', '${twelve ? h % 12 || 12 : h}')`);
    await T(`set('#${id}-m', '${m}')`);
    if (twelve) await T(`set('#${id}-p', '${h >= 12 ? "pm" : "am"}')`);
  };
  const section = async (id: string, label: string) => {
    await b.ev(`location.hash = "#/settings/${id}"`);
    await b.until(`T.text('.tabs a[aria-current="page"]') === ${JSON.stringify(label)}`, `the ${label} section`);
  };
  const viewport = (width: number) =>
    b.send("Emulation.setDeviceMetricsOverride", { width, height: 800, deviceScaleFactor: 1, mobile: false });
  const shot = async (name: string, o: { dark?: boolean; width?: number } = {}) => {
    if (shots)
      await b.screenshot(join(shots, name + ".png"), { width: o.width ?? 390, dark: o.dark ?? false, full: true });
  };

  async function report(): Promise<number> {
    await sleep(50);
    b.close();
    mock.close();
    let failed = 0;
    for (const r of results) {
      console.log(`${r.ok ? "ok  " : "FAIL"} ${r.name}${r.ok ? "" : "\n       " + r.why}`);
      if (!r.ok) failed++;
    }
    console.log(`\nportal browser test: ${results.length - failed}/${results.length} passed`);
    return failed ? 1 : 0;
  }

  return { b, mock, shots, open, T, eq, setTime, section, viewport, shot, step, report };
}
