import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function clock(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, setTime, shot, step } = c;
  await step("Clock: alarms, timers, reminders and the stopwatch each have their own page", async () => {
    await open("open", "#/clock");
    await b.until(`T.all('#clocktabs a').length === 4`, "the Clock tabs");
    await eq(
      `T.all('#clocktabs a').map((a) => a.textContent)`,
      ["Alarms", "Timers", "Reminders", "Stopwatch"],
      "each kind has a tab",
    );
    await eq(`T.text('#clocktabs a[aria-current="page"]')`, "Alarms", "alarms come first");
    // Alarms: only alarms are listed, and the form makes alarms.
    await b.until(`T.rows().length === 1`, "one alarm");
    await shot("clock");
    await eq(`T.rows()[0].includes('<b>work</b>')`, true, "label shown as text");
    await eq(`T.q('#panel li button').getAttribute('aria-label')`, "Edit <b>work</b>", "a row's button says which row");
    await eq(`T.q('#panel strong b') === null`, true, "no markup from the label");
    await eq(`T.rows()[0].includes('Weekdays at')`, true, "repeat text");
    await eq(`T.text('#clocklist h3').startsWith('Alarms')`, true, "the list has a title and a count");
    await eq(`T.q('#rkind') === null`, true, "no type to choose: this page makes alarms");
    await eq(`T.q('#clockdialog').open`, false, "the form is not in the way");
    await T(`click('#addclock')`);
    await b.until(`T.q('#clockdialog').open === true`, "the dialog opens");
    await eq(`T.text('#clockdialog h3')`, "Add an alarm", "it says what it makes");
    await setTime("rtime", "06:30");
    await eq(
      `T.q('#rtime').tagName + T.q('#rtime-m').tagName`,
      "INPUTINPUT",
      "the hour and minute are plain boxes you can type in",
    );
    await eq(`T.q('#rtime').getAttribute('aria-label')`, "Alarm time, hour", "the box is named for what it is");
    // emptying one box leaves no time: the form must not still send the old one
    await T(`set('#rtime-m', '')`);
    const posts = () => mock.log.filter((l) => l.method === "POST" && l.path === "/api/schedule").length;
    const before = posts();
    await T(`submit('#clockdialog form')`);
    await eq(`T.text('#msg')`, "Pick a time for the alarm.", "an incomplete time is refused");
    assert.equal(posts(), before, "nothing was sent");
    await setTime("rtime", "06:30");
    await T(`set('#rlabel', 'gym')`);
    await T(`click('.days input[value="0"]')`);
    await T(`click('.days input[value="2"]')`);
    await b.ev(`(() => { const f = T.q('#clockdialog form'); f.requestSubmit(); f.requestSubmit(); })()`);
    await b.until(`T.rows().length === 2 && T.q('#clockdialog').open === false`, "alarm added, and the dialog closes");
    assert.equal(posts() - before, 1, "one request for a double click");
    const post = mock.log.filter((l) => l.method === "POST" && l.path === "/api/schedule").pop()!;
    assert.deepEqual(post.body, { kind: "alarm", label: "gym", hour: 6, minute: 30, days: [0, 2] });
    // Timers: the timer is listed here and the alarms are not.
    await b.ev(`location.hash = '#/clock/timers'`);
    await b.until(`T.text('#clocklist h3').startsWith('Timers') && T.rows().length === 1`, "the timer");
    await eq(`T.rows()[0].startsWith('pasta') && T.rows()[0].includes('Rings in 10 min')`, true, "timer text");
    await T(`clickText('#panel ul.rows > li:nth-child(1)', 'Edit')`);
    await eq(`T.q('#rlabel').value`, "pasta", "edit loads the label");
    await eq(`T.text('#clockdialog h3')`, "Change this timer", "edit title");
    await T(`set('#rlabel', 'tea')`);
    await T(`set('#ramount', '20')`);
    await T(`submit('#clockdialog form')`);
    await b.until(`T.rows()[0].startsWith('tea')`, "edit saved");
    const put = mock.log.find((l) => l.method === "PUT")!;
    assert.equal(put.path, "/api/schedule/1");
    assert.deepEqual(put.body, { kind: "timer", label: "tea", seconds: 1200 });
    await eq(`T.q('#clockdialog').open`, false, "the editor closed after saving");
    await T(`clickText('#panel ul.rows > li:nth-child(1)', 'Cancel')`);
    await b.until(
      `T.rows().length === 1 && T.q('#clocklist .empty') !== null`,
      "cancelled, and the empty list says what to do",
    );
    assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/schedule/1"));
    // Reminders: validation.
    await b.ev(`location.hash = '#/clock/reminders'`);
    await T(`click('#addclock')`);
    await b.until(`!!T.q('#rdate')`, "the reminder form");
    await T(`set('#rdate', '')`);
    await T(`submit('#clockdialog form')`);
    await eq(`T.text('#msg')`, "Pick a date and a time for the reminder.", "validation message");
    await eq(`(T.text('#clocklist p.mut') ?? '').length > 0`, true, "zone note present");
    // Reminders after a delay (REQ-TMR-009): the same amount-and-unit fields as a timer, listed with time left.
    await T(`set('#rwhen', 'delay')`);
    await b.until(`!!T.q('#ramount') && T.q('#rdate') === null`, "the delay fields replace the date and time");
    await T(`set('#ramount', '15')`);
    await T(`set('#rlabel', 'stand up')`);
    await T(`submit('#clockdialog form')`);
    await b.until(
      `T.rows().length === 1 && T.q('#clockdialog').open === false`,
      "reminder after a delay added, and the dialog closes",
    );
    const rem = mock.log.filter((l) => l.method === "POST" && l.path === "/api/schedule").pop()!;
    assert.deepEqual(rem.body, { kind: "reminder", label: "stand up", seconds: 900 });
    await eq(`T.rows()[0].includes('Says it in 15 min')`, true, "listed with its time left");
    await T(`clickText('#panel ul.rows > li:nth-child(1)', 'Edit')`);
    await eq(`T.q('#rwhen').value`, "delay", "edit opens on the delay choice");
    await eq(`T.q('#ramount').value`, "15", "and the amount");
    await T(`click('#clockdialog .dlgfoot .plain')`);
    await b.until(`T.q('#clockdialog').open === false`, "edit closed");
    // Stopwatch: used by voice, so this page explains and offers nothing that would not work.
    await b.ev(`location.hash = '#/clock/stopwatch'`);
    await b.until(`!!T.q('#stopwatchcard')`, "the stopwatch page");
    await eq(`T.text('#stopwatchcard').includes('Start the stopwatch')`, true, "says how to use it");
    await eq(`T.q('#panel form') === null && T.q('#panel button') === null`, true, "no controls that do nothing");
  });
}
