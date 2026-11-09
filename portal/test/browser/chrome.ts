// A tiny Chrome DevTools Protocol driver with no npm dependencies (Node's built-in WebSocket and fetch).
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export function findChrome(): string | null {
  const env = process.env["CHROME"];
  if (env) return existsSync(env) ? env : null;
  const candidates = [
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  ];
  return candidates.find((c) => existsSync(c)) ?? null;
}

type Params = Record<string, unknown>;
interface CdpException {
  text?: string;
  exception?: { description?: string };
}
/** The parts of a DevTools protocol message (an answer or an event) that this driver reads. */
export interface CdpMessage {
  id?: number;
  error?: { message?: string };
  result?: { exceptionDetails?: CdpException; result?: { value?: unknown }; data?: string };
  method?: string;
  params?: {
    type?: string;
    request?: { url: string };
    exceptionDetails?: CdpException;
    args?: { value?: unknown; description?: string }[];
    entry?: { level: string; text: string; url?: string };
  };
}

export class Browser {
  private ws!: WebSocket;
  private seq = 0;
  private pending = new Map<number, (m: CdpMessage) => void>();
  private proc!: ChildProcess;
  private dir = "";
  /** Every request the page made, every console error, and every uncaught exception. */
  requests: string[] = [];
  problems: string[] = [];

  static async launch(chrome: string): Promise<Browser> {
    const b = new Browser();
    b.dir = mkdtempSync(join(tmpdir(), "portal-chrome-"));
    b.proc = spawn(
      chrome,
      [
        "--headless=new",
        "--remote-debugging-port=0",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-gpu",
        "--disable-extensions",
        "--disable-background-networking",
        "--user-data-dir=" + b.dir,
        "about:blank",
      ],
      { stdio: "ignore" },
    );
    let port = 0;
    for (let i = 0; i < 100 && !port; i++) {
      try {
        port = Number(readFileSync(join(b.dir, "DevToolsActivePort"), "utf8").split("\n")[0]);
      } catch {
        await sleep(100);
      }
    }
    if (!port) {
      b.close();
      throw new Error("Chrome did not start");
    }
    let page: { webSocketDebuggerUrl: string } | undefined;
    for (let i = 0; i < 50 && !page; i++) {
      try {
        page = (
          (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()) as {
            type: string;
            webSocketDebuggerUrl: string;
          }[]
        ).find((t) => t.type === "page");
      } catch {
        /* retry */
      }
      if (!page) await sleep(100);
    }
    if (!page) {
      b.close();
      throw new Error("Chrome has no page");
    }
    b.ws = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise<void>((r, j) => {
      b.ws.onopen = () => r();
      b.ws.onerror = () => j(new Error("devtools socket"));
    });
    b.ws.onmessage = (e) => b.onMessage(JSON.parse(String(e.data)) as CdpMessage);
    await b.send("Page.enable");
    await b.send("Runtime.enable");
    await b.send("Network.enable");
    await b.send("Log.enable");
    return b;
  }

  private onMessage(m: CdpMessage): void {
    const waiting = m.id ? this.pending.get(m.id) : undefined;
    if (m.id && waiting) {
      waiting(m);
      this.pending.delete(m.id);
      return;
    }
    const p = m.params ?? {};
    if (m.method === "Page.loadEventFired") this.onLoad();
    if (m.method === "Network.requestWillBeSent") this.requests.push(p.request?.url ?? "");
    else if (m.method === "Runtime.exceptionThrown")
      this.problems.push("exception: " + (p.exceptionDetails?.exception?.description ?? p.exceptionDetails?.text));
    else if (m.method === "Runtime.consoleAPICalled" && p.type === "error")
      this.problems.push(
        "console.error: " +
          (p.args ?? [])
            .map((a) => (typeof a.value === "string" ? a.value : (a.description ?? JSON.stringify(a.value))))
            .join(" "),
      );
    // A 4xx answer is a normal part of some flows (wrong password, expired session); the page handles those.
    else if (m.method === "Log.entryAdded" && p.entry?.level === "error" && !/status of 4\d\d/.test(p.entry.text))
      this.problems.push("log: " + p.entry.text + " " + (p.entry.url ?? ""));
  }

  send(method: string, params: Params = {}): Promise<CdpMessage> {
    return new Promise((resolve) => {
      const id = ++this.seq;
      this.pending.set(id, resolve);
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  private onLoad: () => void = () => undefined;

  async goto(url: string): Promise<void> {
    const loaded = new Promise<void>((r) => {
      this.onLoad = r;
    });
    await this.send("Page.navigate", { url });
    await Promise.race([loaded, sleep(5000)]);
  }

  /** Evaluate an expression in the page; promises are awaited. */
  async ev<T = unknown>(expr: string): Promise<T> {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    const failure = r.result?.exceptionDetails;
    if (r.error || failure)
      throw new Error(
        `evaluating ${expr.slice(0, 80)}: ${r.error?.message ?? failure?.exception?.description ?? failure?.text}`,
      );
    return r.result?.result?.value as T;
  }

  async until(expr: string, what: string, ms = 4000): Promise<void> {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      try {
        if (await this.ev(expr)) return;
      } catch {
        /* the page may be navigating */
      }
      await sleep(30);
    }
    throw new Error("timed out waiting for " + what);
  }

  async screenshot(file: string, o: { width: number; height?: number; dark?: boolean; full?: boolean }): Promise<void> {
    await this.send("Emulation.setDeviceMetricsOverride", {
      width: o.width,
      height: o.height ?? 900,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await this.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-color-scheme", value: o.dark ? "dark" : "light" }],
    });
    await sleep(120);
    const r = await this.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: !!o.full });
    writeFileSync(file, Buffer.from(r.result?.data ?? "", "base64"));
  }

  close() {
    try {
      this.ws?.close();
    } catch {
      /* already closed */
    }
    this.proc?.kill();
    setTimeout(() => {
      try {
        rmSync(this.dir, { recursive: true, force: true });
      } catch {
        /* chrome may still hold files */
      }
    }, 500).unref();
  }
}

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
