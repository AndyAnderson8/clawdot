import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function settingsNumbers(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, setTime, section, step } = c;
  await step("Settings: numbers show their source and range, save only what changed, reset to defaults", async () => {
    await open("open", "#/settings/assistant");
    await b.until(`!!T.q('#f-agent_max_rounds')`, "agent settings form");
    await eq(
      `T.q('#f-agent_max_rounds').getAttribute('min') + '-' + T.q('#f-agent_max_rounds').getAttribute('max')`,
      "1-20",
      "range is offered to the browser",
    );
    await eq(
      `T.q('#f-agent_max_tool_calls').closest('.srow').textContent.includes('Preset on this device.')`,
      true,
      "a value from the file says so",
    );
    await eq(
      `T.q('#f-agent_max_rounds').closest('.srow').querySelector('.meta') === null`,
      true,
      "a plain default says nothing: no noise",
    );
    await eq(`T.q('#f-plan_speech') === null`, true, "on and off switches live under Features, not here");
    // out of range: the browser stops it before anything is sent
    await T(`set('#f-agent_max_rounds', '99')`);
    await b.ev(`T.q('#f-agent_max_rounds').form.requestSubmit()`);
    await eq(`T.q('#f-agent_max_rounds').validity.rangeOverflow`, true, "the range is enforced in the page");
    assert.ok(!mock.log.some((l) => l.method === "PUT" && l.path === "/api/settings"), "nothing was sent");
    // a good change sends only what changed, with real numbers
    await T(`set('#f-agent_max_rounds', '5')`);
    await b.ev(`T.q('#f-agent_max_rounds').form.requestSubmit()`);
    await b.until(`T.text('#msg') === 'Saved.'`, "saved");
    const put = mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!;
    assert.deepEqual(put.body, { agent_max_rounds: 5 });
    await eq(
      `T.q('#f-agent_max_rounds').closest('.srow').textContent.includes('You changed this here.')`,
      true,
      "source follows",
    );
    // a saved value can go back to its default, for the whole group at once
    await b.until(
      `T.all('.sgroup .head button').some((x) => x.textContent === 'Reset to defaults')`,
      "the reset button",
    );
    await b.ev(`T.q('#f-agent_max_rounds').form.querySelector('.head button').click()`);
    await b.until(`T.q('#f-agent_max_rounds').value === '8'`, "back to the default");
    assert.deepEqual(mock.log.filter((l) => l.method === "PUT").pop()!.body, { agent_max_rounds: null });
    // quiet hours are on the Assistant tab
    await section("assistant", "Assistant");
    // the spending limit is in dollars; there is no limit on how many requests are made
    await T(`set('#f-daily_spend_usd', '2.5')`);
    await b.ev(`T.q('#f-daily_spend_usd').form.requestSubmit()`);
    await b.until(`T.text('#msg') === 'Saved.'`, "the spending limit saved");
    assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
      daily_spend_usd: 2.5,
    });
    await eq(`T.q('#f-quiet_start').getAttribute('inputmode')`, "numeric", "quiet hours use a typed time");
    await eq(`T.q('#f-dry_run') === null`, true, "practice mode is a switch under Features");
    await setTime("f-quiet_start", "22:00");
    await b.ev(`T.q('#f-quiet_start').form.requestSubmit()`);
    await b.until(
      `T.all('.errtext').some((e) => e.textContent.startsWith('Set both the quiet hours'))`,
      "needs both ends",
    );
    await setTime("f-quiet_end", "07:00");
    await b.ev(`T.q('#f-quiet_start').form.requestSubmit()`);
    await b.until(`T.text('#msg') === 'Saved.'`, "quiet hours saved");
    assert.deepEqual(mock.log.filter((l) => l.method === "PUT").pop()!.body, {
      quiet_start: "22:00",
      quiet_end: "07:00",
    });
  });
}
