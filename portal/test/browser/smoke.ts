// Browser smoke test: drives the built page (crates/core/src/ui.html) in headless Chrome against a mock of the
// device API, under the real Content-Security-Policy. Run with `npm run test:browser` after `npm run build`.
//   CHROME=/path/to/chrome  choose the browser (otherwise the usual install paths are tried)
//   SHOTS=dir               also write phone-width screenshots of every screen to dir
// Without a browser the test is skipped locally and fails when CI is set.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { Browser, findChrome, sleep } from "./chrome.ts";
import { PASSWORD, startMock, type Mock } from "./mock-api.ts";

const chrome = findChrome();
if (!chrome) {
  const msg = "portal browser test: no Chrome found (set CHROME=/path/to/chrome)";
  if (process.env["CI"]) {
    console.error(msg);
    process.exit(1);
  }
  console.log(msg + "; skipped");
  process.exit(0);
}

const HELPERS = `(() => {
  const settle = () => new Promise((r) => setTimeout(r, 50));
  window.T = {
    q: (s) => document.querySelector(s),
    all: (s) => [...document.querySelectorAll(s)],
    text: (s) => (document.querySelector(s)?.textContent ?? null),
    settle,
    set: async (s, v) => { const e = document.querySelector(s); if (!e) throw new Error('no ' + s); e.value = v; e.dispatchEvent(new Event('input', { bubbles: true })); e.dispatchEvent(new Event('change', { bubbles: true })); await settle(); },
    click: async (s) => { const e = document.querySelector(s); if (!e) throw new Error('no ' + s); e.click(); await settle(); },
    clickText: async (scope, text) => { const e = [...document.querySelectorAll(scope + ' button, ' + scope + ' a, ' + scope + ' summary')].find((x) => x.textContent.trim() === text); if (!e) throw new Error('no "' + text + '" in ' + scope); e.click(); await settle(); },
    submit: async (s) => { const f = document.querySelector(s); f.requestSubmit(); await settle(); },
    rows: (s) => [...document.querySelectorAll(s ?? '#panel ul.rows > li')].map((li) => li.textContent),
  };
  window.confirm = () => true;
  return true;
})()`;

const results: { name: string; ok: boolean; why?: string }[] = [];
async function step(name: string, fn: () => void | Promise<void>): Promise<void> {
  try {
    await fn();
    results.push({ name, ok: true });
  } catch (e) {
    let page = "";
    try {
      page = await b.ev<string>("location.href + ' | ' + document.body.innerText.replace(/\\s+/g, ' ').slice(0, 160)");
    } catch {
      /* page gone */
    }
    results.push({
      name,
      ok: false,
      why:
        (e as Error).message +
        (page ? "\n       page: " + page : "") +
        (b.problems.length ? "\n       problems: " + b.problems.join("; ") : ""),
    });
  }
}

const mock: Mock = await startMock("open");
const b = await Browser.launch(chrome);
const shots = process.env["SHOTS"];
if (shots) mkdirSync(shots, { recursive: true });

async function open(mode: "setup" | "password" | "open", hash = "", extra: Record<string, unknown> = {}) {
  // Start from a clean browser session on the device's origin, then a clean device.
  await b.goto(mock.url + "/api/auth");
  await b.ev("sessionStorage.clear()");
  mock.reset(mode);
  Object.assign(mock.state, extra);
  await b.goto(mock.url + "/" + hash);
  await b.ev(HELPERS);
}
const T = (expr: string) => b.ev(`T.${expr}`);
const eq = async (expr: string, want: unknown, what: string) => assert.deepEqual(await b.ev(expr), want, what);
const section = async (id: string, label: string) => {
  await b.ev(`location.hash = '#/settings/${id}'`);
  await b.until(`T.text('#sechead') === ${JSON.stringify(label)}`, `the ${label} section`);
};
const viewport = (width: number) =>
  b.send("Emulation.setDeviceMetricsOverride", { width, height: 800, deviceScaleFactor: 1, mobile: false });
const shot = async (name: string, o: { dark?: boolean; width?: number } = {}) => {
  if (shots)
    await b.screenshot(join(shots, name + ".png"), { width: o.width ?? 390, dark: o.dark ?? false, full: true });
};

// ---------------------------------------------------------------- first run and sign-in
await step("setup: mismatched and short passwords are refused, a good one signs in", async () => {
  await open("setup");
  await b.until(`T.text('#setup h2') === 'Set up ClawDot'`, "setup screen");
  await eq(`!!T.q('#hero') && !T.q('#band')`, true, "hero shown, header hidden");
  await shot("setup");
  await T(`set('#pw1', 'longenough1')`);
  await T(`set('#pw2', 'different12')`);
  await T(`submit('#pwform')`);
  await eq(`T.text('#msg')`, "The two passwords differ. Type the same one twice.", "mismatch message");
  await T(`set('#pw1', 'short')`);
  await T(`set('#pw2', 'short')`);
  await T(`submit('#pwform')`);
  await eq(`T.text('#msg')`, "Use at least 8 characters.", "short message");
  await T(`set('#pw1', 'longenough1')`);
  await T(`set('#pw2', 'longenough1')`);
  await T(`submit('#pwform')`);
  await b.until(`!!T.q('#band') && !!T.q('#statusline')`, "the portal after setup");
  const call = mock.log.find((l) => l.path === "/api/setup")!;
  assert.deepEqual(call.body, { mode: "password", password: "longenough1" });
  await eq(`T.q('#pw1') === null`, true, "setup form gone");
  await eq(`T.q('#banner') === null`, true, "no open-mode banner in password mode");
});

await step("setup: leaving it open shows the banner", async () => {
  await open("setup");
  await b.until(`T.q('#openbtn')`, "setup");
  await eq(`T.q('#openbtn').disabled`, false, "open button enabled on a private network");
  await T(`click('#openbtn')`);
  await b.until(`!!T.q('#band')`, "portal");
  await eq(`T.q('#banner')?.getAttribute('role')`, "alert", "banner is an alert");
  await eq(`T.text('#banner').startsWith('Unprotected (open mode)')`, true, "banner text");
  await shot("open-banner");
});

await step(
  "sign-in: wrong password is refused, right one opens the portal, reload keeps the session, sign out returns",
  async () => {
    await open("password");
    await b.until(`T.text('#login h2') === 'Sign in'`, "sign-in screen");
    await shot("login");
    await T(`set('#pw', 'wrong')`);
    await T(`submit('#loginform')`);
    await b.until(
      `T.text('#msg') === 'That password was not accepted. Check it and try again.'`,
      "wrong password message",
    );
    await eq(`T.q('#msg').className`, "toast err", "error styling");
    await T(`set('#pw', ${JSON.stringify(PASSWORD)})`);
    await T(`submit('#loginform')`);
    await b.until(`!!T.q('#band')`, "portal");
    await eq(`T.q('#pw') === null`, true, "sign-in form gone");
    await b.goto(mock.url + "/");
    await b.ev(HELPERS);
    await b.until(`!!T.q('#band')`, "portal again after reload (sessionStorage)");
    await section("security", "Security");
    await T(`clickText('#panel', 'Sign out')`);
    await b.until(`T.text('#login h2') === 'Sign in'`, "sign-in after sign out");
    assert.ok(mock.log.some((l) => l.path === "/api/logout"));
  },
);

await step("an expired session sends you back to sign in", async () => {
  await open("password");
  await b.until(`!!T.q('#pw')`, "sign-in screen");
  await T(`set('#pw', ${JSON.stringify(PASSWORD)})`);
  await T(`submit('#loginform')`);
  await b.until(`!!T.q('#band')`, "portal");
  mock.state["session"] = "other"; // the device forgot us
  await b.ev(`location.hash = '#/reminders'`); // the tab's first fetch is refused
  await b.until(`!!T.q('#login')`, "sign-in screen after a 401");
  await eq(`T.text('#msg')`, "Please sign in again.", "explanation");
});

// ---------------------------------------------------------------- the shell
await step("shell: four destinations, titles, current page, unknown address", async () => {
  await open("open");
  await b.until(`!!T.q('#band') && !!T.q('#statusline')`, "the dashboard");
  await eq(`T.text('#devname')`, "Kitchen Dot", "device name from settings");
  await eq(
    `T.all('#tabs a').map((a) => a.textContent)`,
    ["Dashboard", "Reminders", "Activity", "Settings"],
    "four destinations, Dashboard first (REQ-CFG-026)",
  );
  await eq(`T.all('#tabs a[aria-current="page"]').map((a) => a.id)`, ["tab-dashboard"], "dashboard is current first");
  await b.until(`document.title === "Dashboard - ClawDot"`, "the Dashboard title");
  await T(`click('#tab-settings')`);
  await b.until(
    `document.title === "Settings - ClawDot" && T.q('#tab-settings').getAttribute('aria-current') === 'page'`,
    "settings",
  );
  await eq(`document.activeElement.id`, "panel", "focus moves to the page content after a move");
  await b.ev(`location.hash = '#/nonsense'`);
  await b.until(`!!T.q('#statusline')`, "an unknown address falls back to the Dashboard");
  await eq(`T.text('#ver')`, "ClawDot 0.1.0-test", "footer version");
  await T(`click('#newlink')`);
  await b.until(`T.q('#about')?.open === true`, "the release notes open from the footer link");
  await eq(`T.all('#panel [role="tab"], #panel [role="listbox"], [role="combobox"]').length`, 0, "no custom widgets");
});

await step("Dashboard: live state, attention with fix links, next up, recent, quick add, test speak", async () => {
  await open("open");
  await b.until(`T.text('#statusline') === 'Ready.' && T.all('#attention li').length === 1`, "dashboard");
  await shot("dashboard");
  await shot("dashboard-wide", { width: 1100 });
  await eq(`T.q('#statusblock .ring').getAttribute('data-state')`, "idle", "the ring shows the state");
  await eq(
    `T.text('#statusdetail').startsWith('Last heard')`,
    true,
    "last heard comes from the voice turns in the log",
  );
  await eq(`T.text('#attention li strong')`, "No password", "open mode needs attention");
  await eq(`T.q('#attention li a').getAttribute('href')`, "#/settings/security", "with a fix link");
  await b.until(`T.rows('#nextup ul.rows > li').length === 2`, "two upcoming items");
  await eq(
    `T.rows('#nextup ul.rows > li')[0].includes('Rings in 10 min')`,
    true,
    "next up reads like the Reminders list",
  );
  await b.until(`T.rows('#recent ul.rows > li').length === 6`, "six recent happenings, newest first");
  await eq(`T.all('#recent .badge').length`, 6, "each with an outcome");
  await eq(`T.all('#panel img, #panel script').length`, 0, "no markup from event text");
  // quick add: a timer in one form
  await T(`set('#qamount', '15')`);
  await T(`set('#qlabel', 'tea')`);
  await T(`submit('#quickadd')`);
  await b.until(`T.rows('#nextup ul.rows > li').length === 3`, "timer added");
  assert.deepEqual(mock.log.filter((l) => l.method === "POST" && l.path === "/api/schedule").pop()!.body, {
    kind: "timer",
    label: "tea",
    seconds: 900,
  });
  await T(`clickText('#nextup ul.rows > li:nth-child(3)', 'Cancel')`);
  await b.until(`T.rows('#nextup ul.rows > li').length === 2`, "cancelled from the dashboard");
  // test speak
  await T(`set('#speaktext', 'Hello')`);
  await T(`submit('#sayform')`);
  await b.until(`T.text('#msg') === 'Sent. Listen for the speaker.'`, "toast");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/speak")!.body, { text: "Hello" });
  await eq(`T.q('#speaktext').value`, "", "box cleared");
  await eq(
    `T.all('#sayform button[disabled]').length`,
    2,
    "talk and mute are shown disabled, with the reason beside them",
  );
  await T(`click('#attention li a')`);
  await b.until(`T.text('#sechead') === 'Security'`, "the fix link goes to the right settings section");
});

await step("Dashboard: says why the speaker cannot talk, and where to fix it (REQ-CFG-070)", async () => {
  // The device says so in its status: the box is disabled before anything is tried.
  await open("open", "", {
    statusExtra: { speaker: { connected: false }, voice: { configured: false }, muted: false },
  });
  await b.until(`T.text('#statusline') === 'No speaker is connected.'`, "no speaker");
  await eq(`T.q('#speaktext').disabled && T.q('#saybtn').disabled`, true, "test speak is disabled");
  await eq(`T.text('#sayblock').includes('No voice is set up')`, true, "with the reason");
  await eq(`T.q('#sayfix').getAttribute('href')`, "#/settings/assistant", "and the fix link");
  await eq(
    `T.all('#attention li strong').map((x) => x.textContent)`,
    ["No speaker connected", "No voice set up", "No password"],
    "each problem is listed",
  );
  // The device did not say, but refuses the phrase: the reason it gives is shown with its fix.
  await open("open", "", {
    speakRefusal: { status: 409, reason: "no_speaker", message: "No speaker is connected. Start it." },
  });
  await b.until(`!!T.q('#speaktext') && !T.q('#speaktext').disabled`, "enabled when the status is silent");
  await T(`set('#speaktext', 'Hello')`);
  await T(`submit('#sayform')`);
  await b.until(`!!T.q('#sayblock')`, "the refusal is shown");
  await eq(`T.q('#sayfix').getAttribute('href')`, "#/settings/about", "with its fix link");
  await open("open", "", { statusExtra: { muted: true, quiet_hours: { set: true, active: true } } });
  await b.until(`T.text('#statusline') === 'Muted.'`, "muted");
  await eq(
    `T.all('#attention li strong').map((x) => x.textContent).includes('Speaker muted') && T.all('#attention li strong').map((x) => x.textContent).includes('Quiet hours are on')`,
    true,
    "muted and quiet hours are listed",
  );
});

await step("Reminders: list, edit, cancel, add timer and alarm, validation", async () => {
  await open("open", "#/reminders");
  await b.until(`T.rows().length === 2`, "two rows");
  await shot("reminders");
  await eq(`T.rows()[1].includes('<b>work</b>')`, true, "label shown as text");
  await eq(`T.q('#panel strong b') === null`, true, "no markup from the label");
  await eq(`T.rows()[1].includes('Weekdays at')`, true, "repeat text");
  await eq(`T.rows()[0].startsWith('pasta') && T.rows()[0].includes('Rings in 10 min')`, true, "timer text");
  await T(`clickText('#panel ul.rows > li:nth-child(1)', 'Edit')`);
  await eq(`T.q('#rlabel').value`, "pasta", "edit loads the label");
  await eq(`T.text('#panel form h3')`, "Change this timer", "edit title");
  await T(`set('#rlabel', 'tea')`);
  await T(`set('#ramount', '20')`);
  await T(`submit('#panel form.panel')`);
  await b.until(`T.rows()[0].startsWith('tea')`, "edit saved");
  const put = mock.log.find((l) => l.method === "PUT")!;
  assert.equal(put.path, "/api/schedule/1");
  assert.deepEqual(put.body, { kind: "timer", label: "tea", seconds: 1200 });
  await eq(`T.text('#panel form h3')`, "Add a timer, alarm, or reminder", "form left edit mode");
  await T(`clickText('#panel ul.rows > li:nth-child(2)', 'Cancel')`);
  await b.until(`T.rows().length === 1`, "cancelled");
  assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/schedule/2"));
  await T(`set('#rkind', 'alarm')`);
  await T(`set('#rtime', '06:30')`);
  await T(`set('#rlabel', 'gym')`);
  await T(`click('.days input[value="0"]')`);
  await T(`click('.days input[value="2"]')`);
  await T(`submit('#panel form.panel')`);
  await b.until(`T.rows().length === 2`, "alarm added");
  const post = mock.log.filter((l) => l.method === "POST" && l.path === "/api/schedule").pop()!;
  assert.deepEqual(post.body, { kind: "alarm", label: "gym", hour: 6, minute: 30, days: [0, 2] });
  await T(`set('#rkind', 'reminder')`);
  await T(`set('#rdate', '')`);
  await T(`submit('#panel form.panel')`);
  await eq(`T.text('#msg')`, "Pick a date and a time for the reminder.", "validation message");
  await eq(`T.text('#panel p.mut:not(.row)')?.length > 0`, true, "zone note present");
});

await step("Activity: events from inputs are shown as text", async () => {
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
  await eq(`T.text('#eventlist .mut').includes('input')`, true, "says inputs, not agents");
});

await step("Notes: add, refuse, forget one, forget all after a confirmation, off state", async () => {
  await open("open", "#/settings/notes");
  await b.until(`T.rows("#notelist > li").length === 1`, "one note");
  await shot("settings-notes");
  await eq(`T.rows("#notelist > li")[0].includes("my printer is <b>Pixel</b>")`, true, "note shown literally");
  await eq(`T.all("#panel #notelist b").length`, 0, "no markup from a note");
  await T(`set("#note-text", "the spare key is under the pot")`);
  await T(`submit("#noteform")`);
  await b.until(`T.rows("#notelist > li").length === 2`, "added");
  assert.deepEqual(mock.log.filter((l) => l.method === "POST" && l.path === "/api/notes").pop()!.body, {
    text: "the spare key is under the pot",
  });
  await eq(`T.q("#note-text").value`, "", "box cleared");
  await T(`set("#note-text", "the key is sk-abc")`);
  await T(`submit("#noteform")`);
  await b.until(`T.text("#noteform .errtext").includes("password or a key")`, "a refusal is shown in the form");
  await eq(`T.rows("#notelist > li").length`, 2, "nothing was added");
  // forgetting asks first: declining does nothing
  await b.ev(`window.confirm = () => false`);
  await T(`clickText("#notelist > li:nth-child(1)", "Forget")`);
  await eq(`T.rows("#notelist > li").length`, 2, "declined confirmation keeps the note");
  await b.ev(`window.confirm = () => true`);
  await T(`clickText("#notelist > li:nth-child(1)", "Forget")`);
  await b.until(`T.rows("#notelist > li").length === 1`, "one forgotten");
  assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/notes/1"));
  await T(`click("#forgetall")`);
  await b.until(`!!T.q("#noteempty")`, "all forgotten");
  assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/notes"));
  mock.state["notesOn"] = false;
  await b.ev(`location.hash = "#/dashboard"`);
  await section("notes", "Notes");
  await b.until(`!!T.q("#notesoff")`, "the off notice");
  await eq(`T.q("#note-text").disabled`, true, "adding is disabled while notes are off");
});

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
  await eq(`T.text("#policy").includes("100 actions, 20 of them")`, true, "policy line");
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

await step("Tools: turn off, add a web tool, delete", async () => {
  await open("open", "#/settings/tools");
  await b.until(`T.all('h3').some((h) => h.textContent === 'Web tools (1)')`, "web tools heading");
  await shot("settings-tools");
  await eq(`T.q('#tier-set_light').value`, "costly", "a web tool starts at the careful level");
  await eq(
    `[...T.q('#tier-set_light').options].map((o) => o.value)`,
    ["read_only", "reversible", "costly", "destructive"],
    "choices",
  );
  await eq(`T.q('#tier-set_light option[value="destructive"]').disabled`, true, "destructive is never offered");
  await b.ev(`window.confirm = () => false`);
  await T(`set('#tier-set_light', 'read_only')`);
  assert.ok(!mock.log.some((l) => l.path === "/api/tools/set_light/tier"), "declining the warning sends nothing");
  await b.ev(`window.confirm = () => true`);
  await T(`set('#tier-set_light', 'reversible')`);
  await b.until(
    `T.q('#tier-set_light').value === 'reversible' && T.rows('#part-tools ul.rows > li')[0].includes('Can be undone')`,
    "lowered",
  );
  assert.deepEqual(mock.log.find((l) => l.path === "/api/tools/set_light/tier")!.body, { tier: "reversible" });
  await eq(
    `T.rows('#part-tools ul.rows > li')[0].includes('The usual level is “Asks you first”')`,
    true,
    "says it was changed",
  );
  await T(`set('#tier-set_light', '')`);
  await b.until(`T.q('#tier-set_light').value === 'costly'`, "back to the usual level");
  assert.deepEqual(mock.log.filter((l) => l.path === "/api/tools/set_light/tier").pop()!.body, { tier: null });
  await T(`clickText('#part-tools ul.rows > li:nth-child(1)', 'Turn off')`);
  await b.until(`T.all('.badge.bad').some((x) => x.textContent === 'Off')`, "Off badge");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/tools/set_light/enabled")!.body, { enabled: false });
  await T(`click('#builtin-tools summary')`);
  await eq(`T.q('#builtin-tools').open`, true, "built-in tools open");
  await T(`set('#tool-name', 'lamp')`);
  await T(`set('#tool-desc', 'Lamp')`);
  await T(`set('#tool-url', 'https://h/{room}')`);
  await T(`clickText('#part-tools form.panel', 'Add a value the assistant fills in')`);
  await T(`set('#part-tools form .row input[aria-label="Value name"]', 'room')`);
  await T(`submit('#part-tools form.panel')`);
  await b.until(`T.all('h3').some((h) => h.textContent === 'Web tools (2)')`, "tool added");
  const post = mock.log.find((l) => l.method === "POST" && l.path === "/api/tools")!;
  assert.deepEqual(post.body, {
    name: "lamp",
    description: "Lamp",
    method: "POST",
    url: "https://h/{room}",
    confirm: true,
    params: { room: { type: "string", required: true } },
  });
  await eq(`T.q('#builtin-tools').open`, true, "built-in section stays open across redraws");
  await eq(`T.q('#tool-name').value`, "", "form cleared");
  await T(`clickText('#part-tools ul.rows > li:nth-child(2)', 'Delete')`);
  await b.until(`T.all('h3').some((h) => h.textContent === 'Web tools (1)')`, "deleted");
});

await step("Connections: test, add, turn off, remove", async () => {
  await open("open", "#/settings/tools");
  await b.until(`T.rows('#part-connections > div > ul.rows > li').length === 1`, "one server");

  await eq(`T.text('#part-connections .badge.good')`, "Connected", "status badge");
  await T(`set('#conn-name', 'cal')`);
  await T(`set('#conn-url', 'https://cal.local/mcp')`);
  await T(`clickText('#part-connections form.panel', 'Test connection')`);
  await b.until(
    `T.text('#part-connections form p[role="status"]') === 'Connected. It offers 2 tools: a_tool, b_tool.'`,
    "test result",
  );
  await T(`submit('#part-connections form.panel')`);
  await b.until(`T.rows('#part-connections > div > ul.rows > li').length === 2`, "added");
  assert.deepEqual(mock.log.find((l) => l.method === "POST" && l.path === "/api/connections")!.body, {
    name: "cal",
    url: "https://cal.local/mcp",
    confirm: true,
  });
  await T(`clickText('#part-connections > div > ul.rows > li:nth-child(2)', 'Turn off')`);
  await b.until(`T.all('#part-connections .badge.bad').length === 1`, "Off badge");
  await T(`clickText('#part-connections > div > ul.rows > li:nth-child(2)', 'Remove')`);
  await b.until(`T.rows('#part-connections > div > ul.rows > li').length === 1`, "removed");
});

await step("Skills: add, edit, turn off, export, import, delete", async () => {
  await open("open", "#/settings/tools");
  await b.until(`T.rows('#part-skills ul.rows > li').length === 1`, "one skill");

  await T(`set('#skill-name', 'Morning')`);
  await T(`set('#skill-text', 'Say the weather.')`);
  await T(`set('#skill-tools', 'a, b')`);
  await eq(
    `T.all('#part-skills p.mut').some((p) => p.textContent === '16 of 2000 characters')`,
    true,
    "character count",
  );
  await T(`submit('#part-skills form.panel')`);
  await b.until(`T.rows('#part-skills ul.rows > li').length === 2`, "skill added");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/skills")!.body, {
    name: "Morning",
    instructions: "Say the weather.",
    tools: ["a", "b"],
    enabled: true,
  });
  await T(`clickText('#part-skills ul.rows > li:nth-child(2)', 'Edit')`);
  await eq(`T.q('#skill-tools').value`, "a, b", "edit loads the tools");
  await T(`set('#skill-text', 'Say the weather and the news.')`);
  await T(`submit('#part-skills form.panel')`);
  await b.until(`T.rows('#part-skills ul.rows > li')[1].includes('and the news')`, "edit saved");
  await T(`clickText('#part-skills ul.rows > li:nth-child(2)', 'Turn off')`);
  await b.until(`T.all('#part-skills .badge').filter((x) => x.textContent === 'Off').length === 1`, "turned off");
  await T(`clickText('#part-skills', 'Export')`);
  await eq(`JSON.parse(T.q('textarea[aria-label="Exported skills"]').value).format`, "clawdot-skills", "export JSON");
  await T(`set('textarea[aria-label="Skills file to import"]', 'not json')`);
  await T(`clickText('#part-skills', 'Import as turned off')`);
  await eq(`T.text('#msg')`, "That is not a skills file. Paste the text that Export gave you.", "bad import message");
  await T(
    `set('textarea[aria-label="Skills file to import"]', '{"format":"clawdot-skills","version":1,"skills":[{"name":"Imported","instructions":"x","tools":[]}]}')`,
  );
  await T(`clickText('#part-skills', 'Import as turned off')`);
  await b.until(`T.rows('#part-skills ul.rows > li').length === 3`, "imported");
  await eq(`T.q('textarea[aria-label="Skills file to import"]').value`, "", "import box cleared");
  await T(`clickText('#part-skills ul.rows > li:nth-child(3)', 'Delete')`);
  await b.until(`T.rows('#part-skills ul.rows > li').length === 2`, "deleted");
});

await step("Inputs: create shows the token once, rotate, delete", async () => {
  await open("open", "#/settings/inputs");
  await b.until(`T.rows().length === 1`, "one input");
  await shot("settings-inputs");
  await eq(`T.q('#snip-claude-code').open`, false, "snippets closed");
  await eq(`T.text('#panel li code')`, mock.url + "/hook/claude-code", "address");
  await T(`set('#wname', 'ci')`);
  await T(`set('#wadapter', 'generic')`);
  await T(`submit('#panel form.panel')`);
  await b.until(`T.q('#snip-ci')?.open === true && T.q('#snip-ci code.token')`, "token shown");
  await eq(`T.text('#snip-ci code.token')`, "tok-ci-1", "token");
  await eq(
    `T.all('#snip-ci pre').some((p) => p.textContent.includes('Bearer tok-ci-1'))`,
    true,
    "snippets carry the token",
  );
  await b.ev(`new Promise((r) => setTimeout(r, 100))`);
  await eq(
    `T.all('#snip-claude-code pre').every((p) => p.textContent.includes('YOUR_TOKEN') || !p.textContent.includes('Bearer tok'))`,
    true,
    "other webhooks still use the placeholder",
  );
  await T(`clickText('#snip-ci', 'Copy token')`); // clipboard is unavailable over http; the fallback must not throw
  await T(`clickText('#panel li:nth-child(2)', 'New token')`);
  await b.until(`T.text('#snip-ci code.token') === 'tok-ci-2'`, "rotated token");
  await T(`clickText('#panel li:nth-child(2)', 'Delete')`);
  await b.until(`T.rows().length === 1`, "deleted");
});

await step("Settings: save one group, errors stay in their form, restart notice, password", async () => {
  await open("open", "#/settings/general");
  await b.until(`!!T.q('#f-device_name')`, "settings forms");
  await shot("settings-general");
  await shot("settings-wide", { width: 1100 });
  await eq(`T.text('.soon h4')`, "Restart needed", "restart box");
  await eq(`T.text('.soon p').includes('address this page uses')`, true, "restart list uses plain names");
  await eq(`T.q('#f-zone').tagName`, "SELECT", "zone is a real select");
  await b.ev(`T.q('#f-device_name').form.requestSubmit()`);
  await b.ev("T.settle()");
  await eq(`T.text('#msg')`, "Nothing to save: you have not changed anything here.", "no change, nothing sent");
  assert.ok(!mock.log.some((l) => l.method === "PUT" && l.path === "/api/settings"));
  await T(`set('#f-device_name', 'Den')`);
  await b.ev(`T.q('#f-device_name').form.requestSubmit()`);
  await b.until(`T.text('#devname') === 'Den'`, "header follows the saved name");
  assert.deepEqual(mock.log.find((l) => l.method === "PUT" && l.path === "/api/settings")!.body, {
    device_name: "Den",
  });
  await eq(`T.text('#msg')`, "Saved.", "saved toast");
  await section("assistant", "Assistant");
  await shot("settings-assistant");
  await eq(`T.q('#f-voice').disabled`, true, "voice disabled without a speech provider");
  await T(`set('#f-llm_url', 'http://insecure.example.com')`);
  await b.ev(`T.q('#f-llm_url').form.requestSubmit()`);
  await b.until(
    `T.all('.errtext').some((e) => e.textContent.startsWith('The language model address'))`,
    "inline error",
  );
  await eq(`T.all('.errtext').filter((e) => e.textContent).length`, 1, "one error, in its own form");
  await section("security", "Security");
  await T(`set('#newpw', 'brand new password')`);
  await T(`submit('#pwsec')`);
  await b.until(`T.q('#banner') === null`, "banner gone once a password is set");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/admin/password")!.body, { password: "brand new password" });
  await section("about", "About and diagnostics");
  await T(`click('#about summary')`);
  await b.until(`T.text('#notes')?.includes('First release')`, "release notes loaded");
});

await step("Settings: theme is light unless chosen, remembered, and never follows the system uninvited", async () => {
  await open("open", "#/settings/general");
  await b.until(`!!T.q('#themeform')`, "theme choice");
  await eq(`document.documentElement.dataset.theme`, "light", "light by default");
  // The system prefers dark, and the page stays light.
  await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
  await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(255, 255, 255)", "light on a dark system");
  await eq(
    `T.all('#themeform input[type=radio]').map((r) => r.value + ':' + r.checked)`,
    ["light:true", "dark:false", "system:false"],
    "radios",
  );
  await T(`click('#themeform input[value="system"]')`);
  await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(27, 27, 27)", "match system follows a dark system");
  await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
  await eq(
    `getComputedStyle(document.body).backgroundColor`,
    "rgb(255, 255, 255)",
    "match system follows a light system",
  );
  await T(`click('#themeform input[value="dark"]')`);
  await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(27, 27, 27)", "dark by explicit choice");
  await eq(`localStorage.getItem('clawdot-theme')`, "dark", "remembered by this browser");
  await shot("settings-general-dark", { dark: true });
  await b.goto(mock.url + "/#/settings/general");
  await eq(`document.documentElement.dataset.theme`, "dark", "applied again on load");
  await T(`click('#themeform input[value="light"]')`);
  await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
});

await step("Settings: sections, phone navigation, native controls, no horizontal scroll at 360 px", async () => {
  await open("open", "#/settings");
  await b.until(`T.all('.index a').length === 8`, "the section list");
  await eq(
    `T.all('.index a').map((a) => a.textContent)`,
    ["General", "Assistant", "Inputs", "Tools and connections", "Notes", "Safety", "Security", "About and diagnostics"],
    "every kind of configuration has a named section",
  );
  await viewport(360);
  const shown = (sel: string) => `getComputedStyle(T.q('${sel}')).display !== 'none'`;
  await eq(shown(".sections"), true, "the phone shows the list first");
  await eq(shown("#pane"), false, "and not a section");
  await T(`click('.index a[href="#/settings/safety"]')`);
  await b.until(`T.text('#sechead') === 'Safety'`, "a section");
  await eq(shown(".sections"), false, "the section replaces the list on a phone");
  await T(`click('.back')`);
  await b.until(`location.hash === '#/settings'`, "back to the list");
  const pages = ["#/dashboard", "#/reminders", "#/activity"];
  for (const id of ["general", "assistant", "inputs", "tools", "notes", "safety", "security", "about"])
    pages.push("#/settings/" + id);
  for (const hash of pages) {
    await b.ev(`location.hash = '${hash}'`);
    await b.ev("T.settle()");
    await b.ev("new Promise((r) => setTimeout(r, 250))");
    await eq(
      `document.documentElement.scrollWidth <= window.innerWidth`,
      true,
      `no horizontal scroll at 360 px on ${hash}`,
    );
    // Selects are the platform's own: real elements with the system appearance, no custom popups.
    await eq(
      `T.all('select').every((e) => getComputedStyle(e).appearance !== 'none')`,
      true,
      `native select appearance on ${hash}`,
    );
    await eq(`T.all('[role=listbox], [role=combobox], [aria-haspopup]').length`, 0, `no custom dropdowns on ${hash}`);
  }
  await eq(
    `T.all('input[type=date], input[type=time], input[type=checkbox], input[type=radio], select').every((e) => e.tagName === 'SELECT' || e.tagName === 'INPUT')`,
    true,
    "native inputs",
  );
  await viewport(1100);
});

await step("Settings: agent settings show their source, validate, and save booleans and times", async () => {
  await open("open", "#/settings/assistant");
  await b.until(`!!T.q('#f-agent_max_rounds')`, "agent settings form");
  await eq(
    `T.q('#f-agent_max_rounds').getAttribute('min') + '-' + T.q('#f-agent_max_rounds').getAttribute('max')`,
    "1-20",
    "range is offered to the browser",
  );
  await eq(
    `T.q('#f-agent_max_tool_calls').closest('div').textContent.includes('Configuration file. Applies right away.')`,
    true,
    "source and liveness shown",
  );
  await eq(`T.q('#f-plan_speech').tagName`, "SELECT", "a toggle is a list of On and Off");
  await eq(`T.q('#f-plan_speech').value`, "true", "current value");
  // out of range: the browser stops it before anything is sent
  await T(`set('#f-agent_max_rounds', '99')`);
  await b.ev(`T.q('#f-agent_max_rounds').form.requestSubmit()`);
  await eq(`T.q('#f-agent_max_rounds').validity.rangeOverflow`, true, "the range is enforced in the page");
  assert.ok(!mock.log.some((l) => l.method === "PUT" && l.path === "/api/settings"), "nothing was sent");
  // a good change sends only what changed, with real booleans and numbers
  await T(`set('#f-agent_max_rounds', '5')`);
  await T(`set('#f-plan_speech', 'false')`);
  await b.ev(`T.q('#f-agent_max_rounds').form.requestSubmit()`);
  await b.until(`T.text('#msg') === 'Saved.'`, "saved");
  const put = mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!;
  assert.deepEqual(put.body, { agent_max_rounds: 5, plan_speech: false });
  await eq(`T.q('#f-agent_max_rounds').closest('div').textContent.includes('Saved here.')`, true, "source follows");
  // a saved toggle can go back to its default
  await eq(
    `[...T.q('#f-plan_speech').options].map((o) => o.value)`,
    ["true", "false", ""],
    "default option once saved",
  );
  await T(`set('#f-plan_speech', '')`);
  await b.ev(`T.q('#f-plan_speech').form.requestSubmit()`);
  await b.until(`T.q('#f-plan_speech').value === 'true'`, "back to the default");
  assert.deepEqual(mock.log.filter((l) => l.method === "PUT").pop()!.body, { plan_speech: null });
  // safety: quiet hours and the dry-run flag
  await section("safety", "Safety");
  await eq(`T.q('#f-quiet_start').type`, "time", "quiet hours use a time box");
  await eq(
    `T.q('#f-dry_run').closest('div').textContent.includes('Command-line flag. Applies right away.')`,
    true,
    "a value set by a flag says so",
  );
  await T(`set('#f-quiet_start', '22:00')`);
  await b.ev(`T.q('#f-quiet_start').form.requestSubmit()`);
  await b.until(
    `T.all('.errtext').some((e) => e.textContent.startsWith('Set both the quiet hours'))`,
    "needs both ends",
  );
  await T(`set('#f-quiet_end', '07:00')`);
  await b.ev(`T.q('#f-quiet_start').form.requestSubmit()`);
  await b.until(`T.text('#msg') === 'Saved.'`, "quiet hours saved");
  assert.deepEqual(mock.log.filter((l) => l.method === "PUT").pop()!.body, {
    quiet_start: "22:00",
    quiet_end: "07:00",
  });
});

await step("About: build info, diagnostics download, recent problems, settings export and import", async () => {
  await open("open", "#/settings/about");
  await b.until(`!!T.q('#buildinfo')`, "build information");
  await shot("settings-about");
  await eq(`T.text('#buildinfo')`, "A release build for linux on aarch64. Built with: tz, tls.", "build line");
  // downloads: catch the file instead of saving it
  await b.ev(`(() => {
    window.__files = [];
    URL.createObjectURL = (blob) => { window.__files.push(blob); return 'blob:test'; };
    HTMLAnchorElement.prototype.click = function () { window.__names = (window.__names || []).concat(this.download); };
  })()`);
  await T(`click('#diagdl')`);
  await b.until(`window.__files.length === 1`, "diagnostics offered as a file");
  await eq(`window.__names[0]`, "clawdot-diagnostics.json", "file name");
  await eq(`(async () => JSON.parse(await window.__files[0].text()).format)()`, "clawdot-diagnostics", "file content");
  // recent problems load when the section opens, as text
  await T(`click('#logs summary')`);
  await b.until(`!!T.q('#loglines')`, "log lines");
  await eq(
    `T.text('#loglines').includes('<b>disk full</b>') && T.all('#loglines b').length === 0`,
    true,
    "shown as text",
  );
  // export what is saved here
  await T(`click('#exportdl')`);
  await b.until(`window.__files.length === 2`, "settings offered as a file");
  await eq(
    `(async () => JSON.parse(await window.__files[1].text()).settings.device_name)()`,
    "Kitchen Dot",
    "only saved values",
  );
  // import: a wrong file is explained, a right one is previewed and then sent as one save
  const pick = (text: string) => `(async () => {
    const dt = new DataTransfer();
    dt.items.add(new File([${JSON.stringify(text)}], 'settings.json', { type: 'application/json' }));
    const input = document.querySelector('#importfile');
    input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    await T.settle(); await T.settle();
  })()`;
  await b.ev(pick("{not json"));
  await b.until(`T.text('#importform .errtext').startsWith('That is not a ClawDot settings file')`, "plain refusal");
  await b.ev(
    pick(JSON.stringify({ format: "clawdot-settings", version: 1, settings: { device_name: "Kitchen Dot" } })),
  );
  await b.until(`!!T.q('#importsame')`, "nothing to change");
  await b.ev(
    pick(
      JSON.stringify({
        format: "clawdot-settings",
        version: 1,
        settings: { device_name: "Den", dry_run: false, plan_speech: false },
      }),
    ),
  );
  await b.until(`T.all('#importlist li').length === 3`, "a preview of three changes");
  await eq(
    `T.rows('#importlist li')[0].startsWith('Device name') && T.rows('#importlist li')[0].includes('Kitchen Dot to Den')`,
    true,
    "from and to",
  );
  await T(`submit('#importform')`);
  await b.until(`T.text('#devname') === 'Den'`, "imported and applied");
  assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
    device_name: "Den",
    dry_run: false,
    plan_speech: false,
  });
});

await step("a 10-second refresh keeps what you typed", async () => {
  await open("open", "#/reminders");
  await b.until(`T.rows().length === 2`, "rows");
  await T(`set('#rlabel', 'half typed')`);
  (mock.state["schedule"] as unknown[]).push({
    id: 9,
    kind: "timer",
    label: "new one",
    fire_at: Math.floor(Date.now() / 1000) + 60,
    seconds_left: 60,
    repeat: null,
  });
  await b.until(`T.rows().length === 3`, "the refresh brought the new row", 13000);
  await eq(`T.q('#rlabel').value`, "half typed", "the form was not reset");
});

await step("the page stays inside its own origin and CSP", () => {
  const outside = b.requests.filter(
    (u) => !u.startsWith(mock.url) && !u.startsWith("about:") && !u.startsWith("data:"),
  );
  assert.deepEqual(outside, [], "no request left the device's origin");
  assert.deepEqual(b.problems, [], "no console errors, exceptions, or CSP violations");
});

if (shots)
  await step("light-mode screenshots", async () => {
    await open("setup");
    await b.until(`!!T.q('#setup')`, "setup");
    await shot("setup");
    await open("open", "#/dashboard");
    await b.until(`!!T.q('#statusline')`, "dashboard");
    await shot("dashboard");
  });

await sleep(50);
b.close();
mock.close();
let failed = 0;
for (const r of results) {
  console.log(`${r.ok ? "ok  " : "FAIL"} ${r.name}${r.ok ? "" : "\n       " + r.why}`);
  if (!r.ok) failed++;
}
console.log(`\nportal browser test: ${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
