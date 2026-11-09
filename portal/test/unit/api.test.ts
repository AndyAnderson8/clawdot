import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { api, ApiError, failText, hasToken, keep } from "../../src/api.ts";

interface Call {
  url: string;
  init: RequestInit;
}
let calls: Call[] = [];
const realFetch = globalThis.fetch;

function reply(status: number, body: unknown) {
  globalThis.fetch = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return {
      ok: status >= 200 && status < 300,
      status,
      json: async () => {
        if (body === undefined) throw new Error("no body");
        return body;
      },
    } as Response;
  }) as typeof fetch;
}

beforeEach(() => {
  calls = [];
  keep("");
});
afterEach(() => {
  globalThis.fetch = realFetch;
  keep("");
});

test("a call without a session sends no Authorization header", async () => {
  reply(200, { ok: 1 });
  assert.deepEqual(await api("GET", "/api/status"), { ok: 1 });
  const headers = calls[0]!.init.headers as Record<string, string>;
  assert.equal(headers["Authorization"], undefined);
  assert.equal(calls[0]!.url, "/api/status");
  assert.equal(calls[0]!.init.body, undefined);
});

test("after keep(), calls carry the bearer token and JSON bodies", async () => {
  keep("tok123");
  assert.equal(hasToken(), true);
  reply(200, {});
  await api("POST", "/api/speak", { text: "hi" });
  const headers = calls[0]!.init.headers as Record<string, string>;
  assert.equal(headers["Authorization"], "Bearer tok123");
  assert.equal(headers["Content-Type"], "application/json");
  assert.equal(calls[0]!.init.body, '{"text":"hi"}');
  keep(undefined);
  assert.equal(hasToken(), false);
});

test("errors carry the device's message and status", async () => {
  reply(400, { error: "Timers run from 1 second to 24 hours." });
  await assert.rejects(
    api("POST", "/api/schedule", {}),
    (e: unknown) => e instanceof ApiError && e.status === 400 && e.message === "Timers run from 1 second to 24 hours.",
  );
});

test("an error without a JSON body still says what happened", async () => {
  reply(502, undefined);
  await assert.rejects(
    api("GET", "/api/status"),
    (e: unknown) => e instanceof ApiError && e.status === 502 && e.message === "The device answered with error 502.",
  );
});

test("failText wording", () => {
  assert.deepEqual(failText(new ApiError("x", 401)), { text: "Please sign in again.", signIn: true });
  assert.deepEqual(failText(new ApiError("x", 429)), {
    text: "Too many attempts. Wait a few seconds and try again.",
    signIn: false,
  });
  assert.deepEqual(failText(new ApiError("Nope.", 400)), { text: "Nope.", signIn: false });
  assert.deepEqual(failText("weird"), { text: "Something went wrong. Try again.", signIn: false });
  assert.deepEqual(failText(new Error("")), { text: "Something went wrong. Try again.", signIn: false });
});
