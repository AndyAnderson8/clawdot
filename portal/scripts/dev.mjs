// Live portal for development. Serves the page built fresh from portal/src on every load, reloads the browser when a
// source file changes, and forwards everything else (the API and the event stream) to a running ClawDot core.
//   node scripts/dev.mjs [--port 3340] [--host 127.0.0.1, or 0.0.0.0 to open it from a phone on the same network] [--core http://127.0.0.1:3335]
import * as esbuild from "esbuild";
import { readFileSync, watch } from "node:fs";
import { createServer, request } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = (f) => join(here, "src", f);
const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const port = Number(arg("--port", "3340"));
const host = arg("--host", "127.0.0.1");
const core = new URL(arg("--core", "http://127.0.0.1:3335"));

let version = Date.now();
watch(join(here, "src"), { recursive: true }, () => {
  version = Date.now();
});

// Reload the page when the version changes. Added only here; the real build never contains it.
const reloader = `<script>(()=>{let v;setInterval(async()=>{try{const t=await (await fetch("/__dev/version",{cache:"no-store"})).text();if(v&&t!==v)location.reload();v=t}catch{}},700)})()</script>`;

async function page() {
  const js = await esbuild.build({
    entryPoints: [src("main.tsx")],
    bundle: true,
    format: "iife",
    target: "es2022",
    jsx: "automatic",
    jsxImportSource: "preact",
    write: false,
    sourcemap: "inline",
    define: { "process.env.NODE_ENV": '"development"' },
    logLevel: "silent",
  });
  const css = readFileSync(src("tokens.css"), "utf8") + readFileSync(src("styles.css"), "utf8");
  return readFileSync(src("index.html"), "utf8")
    .replace("/*@css*/", () => css)
    .replace("/*@js*/", () => js.outputFiles[0].text)
    .replace("</body>", () => reloader + "</body>");
}

createServer(async (req, res) => {
  const path = (req.url ?? "/").split("?")[0];
  if (path === "/__dev/version") {
    res.writeHead(200, { "content-type": "text/plain", "cache-control": "no-store" }).end(String(version));
    return;
  }
  if (path === "/" && req.method === "GET") {
    try {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" }).end(await page());
    } catch (e) {
      // A syntax error while typing should show up on the page, not kill the server.
      res.writeHead(500, { "content-type": "text/plain" }).end("portal build failed:\n\n" + String(e?.message ?? e));
    }
    return;
  }
  const headers = { ...req.headers, host: core.host };
  if (headers.origin) headers.origin = core.origin;
  if (headers.referer) headers.referer = core.origin + "/";
  const up = request({ hostname: core.hostname, port: core.port, path: req.url, method: req.method, headers }, (r) => {
    res.writeHead(r.statusCode ?? 502, r.headers);
    r.pipe(res);
  });
  up.on("error", () =>
    res.writeHead(502, { "content-type": "text/plain" }).end(`ClawDot is not reachable at ${core.origin}`),
  );
  req.pipe(up);
}).listen(port, host, () => {
  console.log(
    `portal dev server: http://${host}:${port}/  (API from ${core.origin}; edit portal/src and the page reloads)`,
  );
});
