import assert from "node:assert/strict";
import { test } from "node:test";
import { afterEach, beforeEach } from "node:test";
import { change, deviceNow, get, loadExt, set } from "../../src/store.ts";
import type { Extensions, Overview } from "../../src/types.ts";

test("the device's clock is what it reported, not this browser's", () => {
  const before = deviceNow();
  assert.ok(Math.abs(before - Date.now() / 1000) < 5, "until the device has reported, this browser's clock");
  set({ overview: { now: 1234 } as Overview });
  assert.equal(deviceNow(), 1234);
  set({ overview: null });
});

const realFetch = globalThis.fetch;
let calls: string[] = [];
let pending: { resolve: (v: Response) => void }[] = [];
const listing = (name: string) =>
  ({ tools: [{ name }], skills: [], connections: [], problems: [] }) as unknown as Extensions;
const json = (v: unknown) => ({ ok: true, status: 200, json: () => Promise.resolve(v) }) as unknown as Response;
beforeEach(() => {
  calls = [];
  pending = [];
  globalThis.fetch = ((url: string, init: RequestInit) => {
    calls.push((init.method ?? "GET") + " " + url);
    return new Promise<Response>((resolve) => pending.push({ resolve }));
  }) as typeof fetch;
});
afterEach(() => {
  globalThis.fetch = realFetch;
  set({ ext: null });
});

test("asking for the listing twice at once asks the device once", async () => {
  const a = loadExt();
  const b = loadExt();
  assert.equal(calls.length, 1);
  pending[0]!.resolve(json(listing("one")));
  await Promise.all([a, b]);
  assert.equal(get().ext?.tools?.[0]?.name, "one");
});

test("a slow listing cannot put old data back over a change that has come back", async () => {
  const slow = loadExt(); // started first, answered last
  const changed = change("POST", "/api/tools/x/enabled", { enabled: false });
  pending[1]!.resolve(json(listing("after the change")));
  await changed;
  pending[0]!.resolve(json(listing("before the change")));
  await slow;
  assert.equal(get().ext?.tools?.[0]?.name, "after the change");
});
