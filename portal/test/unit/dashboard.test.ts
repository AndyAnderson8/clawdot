import assert from "node:assert/strict";
import { test } from "node:test";
import {
  attention,
  dailyCounts,
  lastHeard,
  recentItems,
  refusal,
  speakBlock,
  speakerView,
  toolName,
} from "../../src/dashboard.ts";
import type { AgentEvent, AuditRecord, Status } from "../../src/types.ts";

const ready: Status = {
  clock_valid: true,
  llm: { configured: true },
  speaker: { connected: true, available: true },
  voice: { configured: true },
  muted: false,
  quiet_hours: { set: false, active: false },
};

test("the speaker's state is told in words, from the device's own state when it gives one", () => {
  assert.equal(speakerView(null).headline, "Checking the speaker.");
  assert.deepEqual(speakerView(ready), { ring: "idle", headline: "Ready.", detail: "" });
  assert.equal(speakerView({ ...ready, speaker: { ...ready.speaker, state: "listening" } }).ring, "listening");
  assert.equal(speakerView({ ...ready, speaker: { ...ready.speaker, state: "speaking" } }).headline, "Speaking.");
  assert.equal(speakerView({ ...ready, muted: true }).headline, "Muted.");
  assert.equal(speakerView({ ...ready, speaker: { connected: false } }).headline, "Waiting for a speaker");
  assert.equal(speakerView({ ...ready, speaker: { connected: true, available: false } }).headline, "Busy or muted.");
  // An older core that says nothing about the speaker is not reported as a problem.
  assert.equal(speakerView({ llm: { configured: true } }).headline, "Ready.");
});

test("attention lists each problem with where to fix it, most important first, and nothing when all is well", () => {
  assert.deepEqual(attention(ready, "password"), []);
  assert.deepEqual(attention(null, "password"), []);
  const all = attention(
    {
      clock_valid: false,
      llm: { configured: false },
      speaker: { connected: false },
      voice: { configured: false },
      muted: true,
      quiet_hours: { set: true, active: true },
    },
    "open",
  );
  assert.deepEqual(
    all.map((a) => a.id),
    ["llm", "speaker", "voice", "clock", "password", "muted", "quiet"],
  );
  assert.equal(all[0]!.to, "#/settings/assistant");
  assert.equal(all.find((a) => a.id === "password")!.to, "#/settings/security");
  assert.equal(all.find((a) => a.id === "quiet")!.to, "#/settings/safety");
  // A muted speaker is fixed with its own button, so there is no link, but the text says how.
  assert.equal(all.find((a) => a.id === "muted")!.to, undefined);
  assert.match(all.find((a) => a.id === "muted")!.why, /mute button/);
  // Unknown is not flagged.
  assert.deepEqual(attention({ llm: { configured: true } }, "password"), []);
});

test("a test phrase is blocked, with a reason and a fix link, only when the device says it cannot work", () => {
  assert.equal(speakBlock(null), null);
  assert.equal(speakBlock(ready), null);
  assert.equal(speakBlock({ llm: {} }), null);
  assert.equal(speakBlock({ ...ready, voice: { configured: false } })?.reason, "no_voice");
  assert.equal(speakBlock({ ...ready, speaker: { connected: false } })?.reason, "no_speaker");
  assert.equal(speakBlock({ ...ready, muted: true })?.reason, "muted");
  // Speech printed to the terminal needs neither a speaker nor a voice.
  assert.equal(
    speakBlock({ ...ready, voice: { configured: false }, speaker: { connected: true, name: "console" } }),
    null,
  );
  assert.equal(refusal("no_speaker").to, "#/settings/about");
  assert.equal(refusal("no_voice").to, "#/settings/voice");
  assert.equal(refusal("busy").to, undefined);
});

const rec = (id: number, ts: number, over: Partial<AuditRecord> = {}): AuditRecord => ({
  id,
  ts,
  turn: 1,
  trigger: "voice",
  tool: "set_timer",
  tier: "reversible",
  args: {},
  decision: "ran",
  ...over,
});
const ev = (id: number, at: number, title: string): AgentEvent => ({
  id,
  endpoint: "ci",
  source: "build server",
  title,
  text: "",
  state: "new",
  received_at: at,
});

test("recent happenings merge actions and events, newest first, in plain words", () => {
  const items = recentItems([rec(1, 100), rec(2, 300, { decision: "held" })], [ev(7, 200, "<b>Build finished</b>")], 5);
  assert.deepEqual(
    items.map((i) => i.key),
    ["a2", "e7", "a1"],
  );
  assert.equal(items[0]!.title, "Set timer");
  assert.equal(items[0]!.note, "Held");
  assert.equal(items[1]!.note, "from build server");
  assert.equal(items[1]!.title, "<b>Build finished</b>", "event text is kept as text, never interpreted");
  assert.equal(recentItems([rec(1, 100), rec(2, 200), rec(3, 300)], [], 2).length, 2);
});

test("last heard is the newest voice turn, and tool names read as words", () => {
  assert.equal(lastHeard([]), null);
  assert.equal(lastHeard([rec(1, 100, { trigger: "event:ci" }), rec(2, 50), rec(3, 90)]), 90);
  assert.equal(toolName("set_timer"), "Set timer");
  assert.equal(toolName("mcp-home-light"), "Mcp home light");
  assert.equal(toolName(""), "Something");
});

test("daily counts cover fourteen days ending today, count each action on its own day, and ignore older ones", () => {
  const now = Date.UTC(2026, 10, 10, 12, 0, 0) / 1000;
  const rec = (ts: number) => ({ id: 1, ts, tool: "set_timer", trigger: "voice" }) as never;
  const days = dailyCounts([rec(now), rec(now - 60), rec(now - 86400), rec(now - 20 * 86400)], now, "UTC");
  assert.equal(days.length, 14);
  assert.equal(days.at(-1)?.today, true);
  assert.equal(days.at(-1)?.date, "2026-11-10");
  assert.equal(days.at(-1)?.count, 2);
  assert.equal(days.at(-2)?.count, 1);
  assert.equal(
    days.reduce((n, d) => n + d.count, 0),
    3,
    "the old one is not counted",
  );
});

test("the speaker waiting for a device is explained, not just refused", () => {
  const v = speakerView({ llm: { configured: true }, speaker: { connected: false } });
  assert.equal(v.headline, "Waiting for a speaker");
  assert.ok(v.detail.includes("On a Dot"));
});

test("a newer version is a notice with where to read about it, and nothing when there is none", () => {
  const up = {
    available: true,
    latest: { version: "0.2.0", notes: "", page: "", published: "" },
  } as never;
  const list = attention(ready, "password", up);
  assert.deepEqual(
    list.map((a) => [a.id, a.to]),
    [["update", "#/settings/about"]],
  );
  assert.equal(list[0]?.title, "Version 0.2.0 is available");
  assert.deepEqual(attention(ready, "password", { available: false, latest: null } as never), []);
  assert.deepEqual(attention(ready, "password", null), []);
});
