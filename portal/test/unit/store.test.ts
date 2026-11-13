import assert from "node:assert/strict";
import { test } from "node:test";
import { deviceNow, set } from "../../src/store.ts";
import type { Overview } from "../../src/types.ts";

test("the device's clock is what it reported, not this browser's", () => {
  const before = deviceNow();
  assert.ok(Math.abs(before - Date.now() / 1000) < 5, "until the device has reported, this browser's clock");
  set({ overview: { now: 1234 } as Overview });
  assert.equal(deviceNow(), 1234);
  set({ overview: null });
});
