// Drift check between the browser test's fake device and the API description. The fake device (test/browser/mock-api.ts)
// reimplements the device's HTTP API by hand; docs/api/openapi.yaml describes the real one (and the Rust side tests that
// description against its route tables). This fails when the fake answers a method and path the description lacks.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { MOCK_ROUTES } from "../browser/mock-api.ts";

const here = new URL(".", import.meta.url);
const SPEC = readFileSync(new URL("../../../docs/api/openapi.yaml", here), "utf8");
const MOCK_SOURCE = readFileSync(new URL("../browser/mock-api.ts", here), "utf8");
const METHODS = new Set(["get", "post", "put", "delete", "patch", "head"]);

/** `/api/notes/{id}` and `/api/notes/{name}` are the same route: every path parameter becomes `{}`. */
const shape = (path: string) => path.replace(/\{[^}]*\}/g, "{}");

/**
 * The documented routes as `METHOD /path` lines. Read as text, not parsed as YAML: under the top-level `paths:` key,
 * path keys sit at two spaces of indent and method keys at four (the same reading crates/core/tests/openapi.rs uses).
 */
function specRoutes(): Set<string> {
  const out = new Set<string>();
  let inPaths = false;
  let path = "";
  for (const line of SPEC.split(/\r?\n/)) {
    if (!line.startsWith(" ") && line.trim() !== "") {
      inPaths = line.trimEnd() === "paths:";
      continue;
    }
    if (!inPaths) continue;
    const p = /^ {2}(\/\S*):$/.exec(line);
    if (p) {
      path = shape(p[1]!);
      continue;
    }
    const m = /^ {4}([a-z]+):$/.exec(line);
    if (m && METHODS.has(m[1]!)) out.add(`${m[1]!.toUpperCase()} ${path}`);
  }
  return out;
}

const mockRoutes = () => new Set(MOCK_ROUTES.map((r) => `${r.method} ${shape(r.path)}`));

test("the description is read: it has routes", () => {
  const spec = specRoutes();
  assert.ok(spec.size > 40, `only ${spec.size} routes read`);
  assert.ok(spec.has("GET /api/auth") && spec.has("PUT /api/notes/{}"), "known routes are present");
});

test("the fake device answers only routes that openapi.yaml documents", (t) => {
  const spec = specRoutes();
  const mock = mockRoutes();
  const undocumented = [...mock].filter((r) => !spec.has(r)).sort();
  assert.deepEqual(undocumented, [], "the fake device serves routes the description does not have");
  // Not a failure: the device has more routes than the page uses, and the fake does not need them all.
  const missing = [...spec].filter((r) => !mock.has(r)).sort();
  t.diagnostic(`documented but not implemented by the fake device (${missing.length}): ${missing.join(", ")}`);
});

test("every exact path the fake device's handlers match is in its declared route list", () => {
  const declared = mockRoutes();
  const found = [...MOCK_SOURCE.matchAll(/m === "([A-Z]+)" && path === "(\/api\/[^"]*)"/g)].map(
    (x) => `${x[1]!} ${shape(x[2]!)}`,
  );
  assert.ok(found.length > 20, "the handlers were read");
  assert.deepEqual(
    found.filter((r) => !declared.has(r)),
    [],
    "a handler matches a route that MOCK_ROUTES does not list",
  );
});

test("the declared route list has no duplicates", () => {
  assert.equal(mockRoutes().size, MOCK_ROUTES.length);
});
