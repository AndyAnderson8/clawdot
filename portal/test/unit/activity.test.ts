import assert from "node:assert/strict";
import { test } from "node:test";
import {
  argsText,
  auditQuery,
  kb,
  NO_FILTERS,
  outcome,
  relaxWarning,
  tierChoice,
  tierLabel,
  triggerLabel,
} from "../../src/activity.ts";

// REQ-CFG-060
test("every decision the core writes has a plain label", () => {
  const decisions = [
    "ran",
    "held",
    "proposed",
    "approved",
    "declined",
    "expired",
    "cancelled",
    "blocked:cap",
    "blocked:dry-run",
    "blocked:invalid",
    "blocked:pending",
    "blocked:limit",
  ];
  const labels = decisions.map((decision) => outcome({ decision }));
  for (const o of labels) {
    assert.ok(o.label && !o.label.includes(":"), o.label);
    assert.ok(o.text.endsWith("."), o.text);
  }
  assert.equal(new Set(decisions.map((d) => outcome({ decision: d }).text)).size, decisions.length, "all distinct");
});

test("a call that ran with an error is shown as failed, and an unknown decision as written", () => {
  assert.equal(outcome({ decision: "ran", status: "error" }).label, "Failed");
  assert.equal(outcome({ decision: "ran", status: "ok" }).tone, "good");
  assert.equal(outcome({ decision: "approved", status: "error" }).tone, "bad");
  assert.equal(outcome({ decision: "from-the-future" }).label, "from-the-future");
  assert.equal(outcome({ decision: "held" }).tone, "warn");
});

test("tiers and triggers read in plain words", () => {
  assert.equal(tierLabel("costly"), "Asks you first");
  assert.equal(tierLabel("odd"), "odd");
  assert.equal(triggerLabel("voice"), "You asked by voice");
  assert.equal(triggerLabel("event:claude-code"), "Event from claude-code");
});

test("the query carries only the filters that are set, trimmed, and the page position", () => {
  assert.equal(auditQuery(NO_FILTERS, null), "limit=25");
  assert.equal(
    auditQuery({ tool: " set_timer ", tier: "costly", decision: "blocked:", trigger: "" }, 41, 10),
    "limit=10&before=41&tool=set_timer&tier=costly&decision=blocked%3A",
  );
});

test("arguments are listed one per line, as text", () => {
  assert.equal(
    argsText({ label: "<b>tea</b>", seconds: 300, nested: { a: 1 } }),
    'label: <b>tea</b>\nseconds: 300\nnested: {"a":1}',
  );
  assert.equal(argsText(null), "");
  assert.equal(argsText({}), "");
  assert.equal(argsText("loose"), '"loose"');
});

test("sizes", () => {
  assert.equal(kb(900), "900 B");
  assert.equal(kb(262144), "256.0 KB");
});

// REQ-SEC-030
test("relaxing a tool asks first, raising it does not", () => {
  assert.ok(relaxWarning("set_light", "costly", "reversible").startsWith("Let set_light run right away"));
  assert.ok(relaxWarning("set_light", "costly", "read_only"));
  assert.equal(relaxWarning("set_light", "reversible", "costly"), "");
  assert.equal(relaxWarning("set_light", "costly", "costly"), "");
  assert.equal(tierChoice("costly"), "Asks you out loud first");
  assert.equal(tierChoice("odd"), "odd");
});
