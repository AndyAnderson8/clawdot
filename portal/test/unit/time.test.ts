import assert from "node:assert/strict";
import { test } from "node:test";
import {
  ago,
  DAYS,
  fmt,
  hhmm,
  parts,
  repeatText,
  span,
  whenText,
  zoneName,
  zoneOptions,
  offsetText,
} from "../../src/time.ts";

// 2026-11-09 12:00:00 UTC
const NOON = Date.UTC(2026, 10, 9, 12, 0, 0) / 1000;

test("span words", () => {
  assert.equal(span(45), "45 s");
  assert.equal(span(600), "10 min");
  assert.equal(span(7200), "2 h");
  assert.equal(span(5400 + 1800), "2 h");
  assert.equal(span(3 * 3600 + 1200), "3 h 20 min");
  assert.equal(span(3 * 86400), "3 days");
});

test("ago", () => {
  assert.equal(ago(NOON - 10, NOON), "just now");
  assert.equal(ago(NOON + 10, NOON), "just now");
  assert.equal(ago(NOON - 600, NOON), "10 min ago");
});

test("parts uses the device zone, not the browser's", () => {
  assert.deepEqual(parts(NOON, "UTC"), { date: "2026-11-09", time: "12:00" });
  assert.deepEqual(parts(NOON, "America/New_York"), { date: "2026-11-09", time: "07:00" });
  assert.deepEqual(parts(NOON, "Pacific/Auckland"), { date: "2026-11-10", time: "01:00" });
});

test("an unknown zone name falls back instead of throwing", () => {
  assert.match(parts(NOON, "Not/AZone").date, /^\d{4}-\d{2}-\d{2}$/);
  assert.ok(fmt(NOON, "Not/AZone", { hour: "numeric" }).length > 0);
});

test("whenText says Today and Tomorrow in the device zone", () => {
  assert.match(whenText(NOON + 3600, NOON, "UTC"), /^Today at /);
  assert.match(whenText(NOON + 86400, NOON, "UTC"), /^Tomorrow at /);
  assert.match(whenText(NOON + 3 * 86400, NOON, "UTC"), /^[A-Z][a-z]{2}, .* at /);
  // 20:00 UTC is already the next day in Auckland.
  assert.match(whenText(NOON + 8 * 3600, NOON + 8 * 3600 - 7200, "Pacific/Auckland"), /^Today at /);
});

test("repeatText", () => {
  const r = (days: number[], every_day = false) => ({ hour: 7, minute: 0, days, every_day });
  assert.equal(repeatText(r([0, 1, 2, 3, 4, 5, 6], true)), "Every day");
  assert.equal(repeatText(r([0, 1, 2, 3, 4])), "Weekdays");
  assert.equal(repeatText(r([5, 6])), "Weekends");
  assert.equal(repeatText(r([0, 2])), "Mon, Wed");
  assert.equal(DAYS.length, 7);
});

test("small formatters", () => {
  assert.equal(hhmm(7, 5), "07:05");
  assert.equal(hhmm(17, 30), "17:30");
  assert.equal(zoneName("America/New_York"), "America/New York");
});

test("zones carry their offset, sort west to east, and say when the offset moves with daylight saving", () => {
  const now = new Date(Date.UTC(2026, 10, 10, 12));
  const opts = zoneOptions(["Europe/Berlin", "America/Panama", "UTC", "Asia/Kolkata", "America/Los_Angeles"], now);
  assert.deepEqual(
    opts.map((o) => o.id),
    ["America/Los_Angeles", "America/Panama", "UTC", "Europe/Berlin", "Asia/Kolkata"].sort(
      (a, b) => opts.find((o) => o.id === a)!.offset - opts.find((o) => o.id === b)!.offset,
    ),
  );
  const label = (id: string) => opts.find((o) => o.id === id)?.label;
  assert.equal(label("America/Panama"), "(UTC-05:00) America/Panama", "no daylight saving, so no note");
  assert.equal(label("UTC"), "(UTC) UTC");
  assert.equal(label("Asia/Kolkata"), "(UTC+05:30) Asia/Kolkata");
  assert.ok(label("Europe/Berlin")?.includes("changes with daylight saving"));
  assert.ok(label("America/Los_Angeles")?.includes("changes with daylight saving"));
  assert.equal(offsetText(-300), "UTC-05:00");
  assert.equal(offsetText(0), "UTC");
});

test("Tomorrow is the next calendar day, even on a day that is not 24 hours long", () => {
  // New York falls back on 2026-11-01: that day has 25 hours, so now + 86400 s is still the same day.
  const now = Date.UTC(2026, 10, 1, 4, 30) / 1000; // 00:30 EDT on the 1st
  const next = Date.UTC(2026, 10, 2, 13, 0) / 1000; // 08:00 EST on the 2nd
  assert.match(whenText(next, now, "America/New_York"), /^Tomorrow at /);
  // and the other way: the spring-forward day has 23 hours
  const spring = Date.UTC(2026, 2, 8, 5, 30) / 1000; // 00:30 EST on 2026-03-08
  const after = Date.UTC(2026, 2, 9, 12, 0) / 1000; // 08:00 EDT on the 9th
  assert.match(whenText(after, spring, "America/New_York"), /^Tomorrow at /);
});
