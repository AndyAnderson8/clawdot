// Drift check for the shapes of the fake device's answers. mock-routes.test.ts checks that the fake serves only routes
// docs/api/openapi.yaml documents; this checks that, for the routes it answers, every top-level key of the JSON body it
// sends is a property the description lists for that route and status. A key the fake sends that the description lacks
// means the fake invented it or the description is behind the real API; either way one of them is wrong.
//
// The YAML is read as text, as mock-routes.test.ts does (the portal has no YAML parser): only the structure this needs
// is read, by indentation. A response schema is a `$ref` to a component or an inline `properties:` block; a schema
// that allows any key (`additionalProperties`, no `properties`) accepts every key.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { MOCK_ROUTES, PASSWORD, startMock } from "../browser/mock-api.ts";

const SPEC = readFileSync(new URL("../../../docs/api/openapi.yaml", import.meta.url), "utf8").split(/\r?\n/);

const indentOf = (l: string) => l.length - l.trimStart().length;
const blank = (l: string) => l.trim() === "";

/** The lines nested under line `at` (deeper than it), up to the next line that is not. */
function nested(lines: string[], at: number): string[] {
  const base = indentOf(lines[at]!);
  const out: string[] = [];
  for (let i = at + 1; i < lines.length; i++) {
    const l = lines[i]!;
    if (!blank(l) && indentOf(l) <= base) break;
    out.push(l);
  }
  return out;
}

/** The direct children of a block: the lines at its first line's indent, with their own nested lines after each. */
function children(block: string[]): { key: string; rest: string; body: string[] }[] {
  const first = block.find((l) => !blank(l));
  if (first === undefined) return [];
  const n = indentOf(first);
  const out: { key: string; rest: string; body: string[] }[] = [];
  for (let i = 0; i < block.length; i++) {
    const l = block[i]!;
    if (blank(l) || indentOf(l) !== n) continue;
    const m = /^\s*("?[^\s:"]+"?):\s*(.*)$/.exec(l);
    if (m) out.push({ key: m[1]!.replaceAll('"', ""), rest: m[2]!, body: nested(block, i) });
  }
  return out;
}

const component = (name: string): string[] => {
  const at = SPEC.findIndex((l) => l === `    ${name}:`);
  assert.ok(at >= 0, `components.schemas.${name} not found`);
  return nested(SPEC, at);
};

const sharedResponse = (name: string): string[] => {
  const section = SPEC.findIndex((l) => l === "  responses:");
  const at = SPEC.findIndex((l, i) => i > section && l === `    ${name}:`);
  assert.ok(section >= 0 && at >= 0, `components.responses.${name} not found`);
  return nested(SPEC, at);
};

/** The `#/components/schemas/Name` a flow value or line holds, if any. */
const refIn = (text: string) => /\$ref:\s*"?#\/components\/schemas\/(\w+)"?/.exec(text)?.[1];

/** The top-level property names a schema allows, or "any" when it allows any key. */
function keysOf(schema: string[], flow = ""): Set<string> | "any" {
  const ref = refIn(flow) ?? schema.map(refIn).find((r) => r);
  const kids = children(schema);
  const props = kids.find((k) => k.key === "properties");
  if (ref && !props) return keysOf(component(ref));
  if (props) return new Set(children(props.body).map((k) => k.key));
  if (kids.some((k) => k.key === "additionalProperties" && k.rest !== "false")) return "any";
  return new Set();
}

/** The keys the description lists for the body of `method path` answered with `status`, or undefined if it says nothing. */
function documented(method: string, path: string, status: number): Set<string> | "any" | undefined {
  const pathAt = SPEC.findIndex((l) => l === `  ${path}:`);
  if (pathAt < 0) return undefined;
  const route = nested(SPEC, pathAt);
  const op = children(route).find((k) => k.key === method.toLowerCase());
  const responses = op && children(op.body).find((k) => k.key === "responses");
  const resp = responses && children(responses.body).find((k) => k.key === String(status));
  if (!resp) return undefined;
  // A shared response is a `$ref` to components.responses; read its block instead.
  const shared = /\$ref:\s*"?#\/components\/responses\/(\w+)"?/.exec(resp.rest)?.[1];
  const content = children(shared ? sharedResponse(shared) : resp.body).find((k) => k.key === "content");
  const json = content && children(content.body).find((k) => k.key === "application/json");
  const schema = json && children(json.body).find((k) => k.key === "schema");
  if (!schema) return undefined;
  return keysOf(schema.body, schema.rest);
}

/** What the fake sends in answer to each request, to compare with the description. */
const SAMPLE_REQUESTS: [method: string, path: string, body?: unknown][] = [
  ["POST", "/api/endpoints", { name: "shape-check", adapter: "claude-code" }],
  ["PUT", "/api/endpoints/shape-check", { spoken_name: "the check" }],
  ["POST", "/api/endpoints/shape-check/rotate"],
  ["PUT", "/api/settings", { device_name: "Den" }],
  ["POST", "/api/notes", { text: "the shed key is blue" }],
  ["POST", "/api/schedule", { kind: "timer", label: "tea", seconds: 300 }],
  ["POST", "/api/admin/password", { password: "another password" }],
];

test("the description is read: known routes give their keys", () => {
  assert.deepEqual([...(documented("GET", "/api/status", 200) as Set<string>)].sort(), ["now", "status"]);
  assert.ok((documented("GET", "/api/settings", 200) as Set<string>).has("fields"));
  assert.ok((documented("POST", "/api/endpoints", 201) as Set<string>).has("token"));
  assert.equal(documented("GET", "/api/nothing", 200), undefined);
});

test("every top-level key of the fake device's answers is documented for that route", async (t) => {
  const mock = await startMock("open");
  try {
    const checked: string[] = [];
    const unchecked: string[] = [];
    const problems: string[] = [];
    const omitted: string[] = [];
    const call = async (method: string, path: string, body?: unknown) => {
      const res = await fetch(mock.url + path, {
        method,
        headers: { "content-type": "application/json", authorization: `Bearer ${PASSWORD}` },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
      const text = await res.text();
      const label = `${method} ${path}`;
      let json: unknown;
      try {
        json = JSON.parse(text);
      } catch {
        unchecked.push(`${label} (not JSON)`);
        return;
      }
      if (res.status >= 300 || typeof json !== "object" || json === null || Array.isArray(json)) {
        unchecked.push(`${label} (${res.status})`);
        return;
      }
      const route = MOCK_ROUTES.find((r) => r.method === method && path.match(pattern(r.path)))?.path ?? path;
      const keys = documented(method, route, res.status);
      if (keys === undefined) {
        problems.push(`${label} answers ${res.status}, which the description does not give a body for`);
        return;
      }
      checked.push(label);
      if (keys === "any") return;
      const extra = Object.keys(json).filter((k) => !keys.has(k));
      if (extra.length)
        problems.push(`${label} (${res.status}): the fake sends ${extra.join(", ")}, not in the description`);
      // Not a failure: the fake need not send what the page does not read.
      const absent = [...keys].filter((k) => !(k in json));
      if (absent.length) omitted.push(`${label}: ${absent.join(", ")}`);
    };
    for (const r of MOCK_ROUTES) if (r.method === "GET" && !r.path.includes("{")) await call("GET", r.path);
    for (const [method, path, body] of SAMPLE_REQUESTS) await call(method, path, body);
    t.diagnostic(`checked ${checked.length}: ${checked.join(", ")}`);
    t.diagnostic(`documented but not sent by the fake (${omitted.length}): ${omitted.join("; ")}`);
    t.diagnostic(`not checked (${unchecked.length}): ${unchecked.join(", ")}`);
    assert.ok(checked.length >= 20, `only ${checked.length} answers were compared`);
    assert.deepEqual(problems, [], "the fake device's answers differ from docs/api/openapi.yaml");
  } finally {
    mock.close();
  }
});

/** A regular expression for a route written with `{name}` parameters. */
function pattern(route: string): RegExp {
  return new RegExp("^" + route.replace(/\{[^}]*\}/g, "[^/]+") + "$");
}
