import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function webhookMessages(c: Ctx): Promise<void> {
  const { b, open, eq, step } = c;
  await step("Activity: messages from webhooks are shown as text", async () => {
    await open("open", "#/activity");
    await b.until(`T.rows('#eventlist > li').length === 1`, "one event");
    await eq(
      `T.rows('#eventlist > li')[0].includes('<img src=x onerror=alert(1)>Build done')`,
      true,
      "title shown literally",
    );
    await eq(
      `T.rows('#eventlist > li')[0].includes('<script>alert(2)</script> all green')`,
      true,
      "text shown literally",
    );
    await eq(`T.all('#eventlist img, #eventlist script').length`, 0, "no elements were created from it");
    await eq(`T.text('#eventlist .badge')`, "Waiting to be read out", "state wording");
    await eq(`T.text('#eventlist .mut').includes('webhook')`, true, "says webhooks, not agents");
  });
}

export async function activityLog(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, shot, step } = c;
  await step("Activity: plain outcomes, filters, paging, text only, read-only", async () => {
    await open("open", "#/activity");
    await b.until(`T.rows("#auditlist > li").length === 7`, "seven records");
    await shot("activity");
    await eq(`T.rows("#auditlist > li")[0].startsWith("fetch_page")`, true, "newest first");
    await eq(
      `T.rows("#auditlist > li")[0].includes("<script>alert(3)</script> long page")`,
      true,
      "result shown literally",
    );
    await eq(`T.all("#panel img, #panel script, #auditlist b").length`, 0, "no elements were created from record text");
    await T(`clickText("#auditlist > li:nth-child(1)", "Show the longer result")`);
    await eq(
      `T.q("#auditlist details").open && T.text("#auditlist details pre").includes("<img src=x")`,
      true,
      "long detail",
    );
    await eq(
      `T.all("#auditlist .badge").map((x) => x.textContent)`,
      ["Ran", "Suggested", "Blocked", "Declined", "Approved", "Held", "Ran"],
      "outcome labels",
    );
    await eq(`T.rows("#auditlist > li")[5].includes("waiting for your yes")`, true, "held is explained");
    await eq(`T.rows("#auditlist > li")[5].includes("token: [redacted]")`, true, "redacted argument shown");
    await eq(
      `T.all("#panel button").map((x) => x.textContent).filter((x) => /delete|clear all|remove/i.test(x)).length`,
      0,
      "read-only",
    );
    await T(`set("#a-decision", "blocked:")`);
    await b.until(`T.rows("#auditlist > li").length === 1`, "blocked filter");
    await T(`set("#a-decision", "")`);
    await T(`set("#a-tier", "reversible")`);
    await b.until(
      `T.rows("#auditlist > li").length === 1 && T.rows("#auditlist > li")[0].startsWith("set_timer")`,
      "tier filter",
    );
    await T(`set("#a-tier", "")`);
    await T(`set("#a-trigger", "event")`);
    await b.until(
      `T.rows("#auditlist > li").length === 1 && T.rows("#auditlist > li")[0].includes("Event from claude-code")`,
      "trigger filter",
    );
    await T(`clickText("#auditfilter", "Clear filters")`);
    await b.until(`T.rows("#auditlist > li").length === 7`, "cleared");
    await T(`set("#a-tool", "set_light")`);
    await T(`submit("#auditfilter")`);
    await b.until(`T.rows("#auditlist > li").length === 3`, "tool filter");
    const q = mock.log.filter((l) => l.path === "/api/audit").pop()!;
    assert.equal(q.method, "GET");
    assert.ok(
      mock.log.every((l) => l.path !== "/api/audit" || l.method === "GET"),
      "only reads",
    );
  });

  await step("Activity: older records load a page at a time", async () => {
    await open("open", "#/activity");
    await b.until(`T.rows("#auditlist > li").length === 7`, "seven records");
    await eq(`T.q("#auditmore") === null`, true, "no more button when all fit");
    const more = Array.from({ length: 33 }, (_, i) => ({
      v: 1,
      id: 8 + i,
      ts: 1,
      day: 1,
      turn: 8 + i,
      trigger: "voice",
      tool: "set_timer",
      tier: "reversible",
      args: {},
      decision: "ran",
      status: "ok",
    }));
    (mock.state["audit"] as unknown[]).push(...more);
    await T(`clickText("#auditfilter", "Refresh")`);
    await b.until(`T.rows("#auditlist > li").length === 25 && !!T.q("#auditmore")`, "first page of 25");
    await T(`click("#auditmore")`);
    await b.until(`T.rows("#auditlist > li").length === 40 && T.q("#auditmore") === null`, "all 40, no more button");
  });
}

export async function logs(c: Ctx): Promise<void> {
  const { b, open, T, eq, step } = c;
  await step(
    "Logs: activity, problems with search and a diagnostics file, and conversations that are not kept",
    async () => {
      await open("open", "#/logs");
      await b.until(`!!T.q('#logtabs')`, "the logs page");
      await eq(
        `T.all('#logtabs a').map((a) => a.textContent)`,
        ["Activity", "Problems", "Conversations"],
        "three pages",
      );
      await eq(`T.text('#panel h2')`, "Logs", "one title");
      await T(`click('#logtabs a[href="#/logs/problems"]')`);
      await b.until(`!!T.q('#loglines')`, "the problems");
      await eq(
        `T.text('#loglines').includes('<b>disk full</b>') && T.all('#loglines b').length === 0`,
        true,
        "shown as text",
      );
      await T(`set('#logsearch', 'nothing like this')`);
      await b.until(`!!T.q('#lognone')`, "a search that matches nothing says so");
      await T(`set('#logsearch', '')`);
      await T(`set('#loglevel', 'warn')`);
      await b.until(`!!T.q('#lognone')`, "no warnings among the errors");
      await T(`set('#loglevel', '')`);
      await b.ev(`(() => {
    window.__files = [];
    URL.createObjectURL = (blob) => { window.__files.push(blob); return 'blob:test'; };
    HTMLAnchorElement.prototype.click = function () { window.__names = (window.__names || []).concat(this.download); };
  })()`);
      await T(`click('#diagdl')`);
      await b.until(`window.__files.length === 1`, "diagnostics offered as a file");
      await eq(`window.__names[0]`, "clawdot-diagnostics.json", "file name");
      await eq(
        `(async () => JSON.parse(await window.__files[0].text()).format)()`,
        "clawdot-diagnostics",
        "file content",
      );
      await b.ev(`location.hash = '#/logs/conversations'`);
      await b.until(`!!T.q('#convcard')`, "conversations");
      await eq(`T.text('#convcard .blank h3')`, "Not recorded", "says plainly that nothing is kept");
      await b.ev(`location.hash = '#/activity'`);
      await b.until(
        `T.q('#logtabs a[aria-current="page"]')?.textContent === 'Activity'`,
        "the old Activity address lands on Logs",
      );
    },
  );
}
