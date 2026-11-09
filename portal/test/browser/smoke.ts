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
async function step(name: string, fn: () => Promise<void>) {
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

async function open(mode: "setup" | "password" | "open", hash = "") {
  // Start from a clean browser session on the device's origin, then a clean device.
  await b.goto(mock.url + "/api/auth");
  await b.ev("sessionStorage.clear()");
  mock.reset(mode);
  await b.goto(mock.url + "/" + hash);
  await b.ev(HELPERS);
}
const T = (expr: string) => b.ev(`T.${expr}`);
const eq = async (expr: string, want: unknown, what: string) => assert.deepEqual(await b.ev(expr), want, what);
const tab = async (id: string, heading: string) => {
  await b.ev(`location.hash = '#/${id}'`);
  await b.until(`T.text('#panel h2') === ${JSON.stringify(heading)}`, `the ${heading} tab`);
};
const shot = async (name: string, dark = true) => {
  if (shots)
    await b.screenshot(join(shots, name + (dark ? "-dark" : "-light") + ".png"), { width: 500, dark, full: true });
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
  await b.until(`!!T.q('#band') && T.text('#panel h2') === 'Home'`, "the portal after setup");
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
  await shot("open-banner", false);
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
    await tab("settings", "Settings");
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
  (mock.state as any).session = "other"; // the device forgot us
  await b.ev(`location.hash = '#/reminders'`); // the tab's first fetch is refused
  await b.until(`!!T.q('#login')`, "sign-in screen after a 401");
  await eq(`T.text('#msg')`, "Please sign in again.", "explanation");
});

// ---------------------------------------------------------------- the shell
await step("header, status, tabs, titles, keyboard", async () => {
  await open("open");
  await b.until(`!!T.q('#band') && T.text('#statetext') === 'Ready'`, "ready");
  await eq(`T.text('#devname')`, "Kitchen Dot", "device name from settings");
  await eq(
    `T.all('#tabs a').map((a) => a.textContent)`,
    ["Home", "Reminders", "Agents", "Tools", "Connections", "Skills", "Webhooks", "Settings"],
    "tab names (REQ-CFG-026)",
  );
  await eq(`T.q('#tabs').getAttribute('role')`, "tablist", "tablist role");
  await eq(`T.all('#tabs a[aria-selected="true"]').map((a) => a.id)`, ["tab-home"], "home selected first");
  await eq(`T.all('#tabs a').map((a) => a.tabIndex)`, [0, -1, -1, -1, -1, -1, -1, -1], "roving tab index");
  await eq(`T.q('#panel').getAttribute('aria-labelledby')`, "tab-home", "panel labelled by its tab");
  await b.until(`document.title === "Home - ClawDot"`, "the Home title");
  // arrow keys move through the tabs, wrapping, and focus follows
  const key = async (k: string, hash: string) => {
    await b.ev(`T.q('#tabs').dispatchEvent(new KeyboardEvent('keydown', { key: '${k}', bubbles: true }))`);
    await b.until(
      `location.hash === '${hash}' && document.activeElement.id === 'tab-${hash.slice(2)}' && T.q('#tab-${hash.slice(2)}').getAttribute('aria-selected') === 'true'`,
      `${k} to ${hash}`,
    );
  };
  await b.ev(`T.q('#tab-home').focus()`);
  await key("ArrowRight", "#/reminders");
  await key("End", "#/settings");
  await key("ArrowRight", "#/home");
  await key("ArrowLeft", "#/settings");
  await b.until(`document.title === "Settings - ClawDot"`, "the title follows the tab");
  await b.ev(`location.hash = '#/nonsense'`);
  await b.until(`T.text('#panel h2') === 'Home'`, "an unknown tab falls back to Home");
  await eq(`T.text('#ver')`, "ClawDot 0.1.0-test", "footer version");
  await T(`click('#newlink')`);
  await b.until(`T.q('#about')?.open === true`, "the release notes open from the footer link");
});

// ---------------------------------------------------------------- tabs
await step("Home: checklist and test speak", async () => {
  await open("open");
  await b.until(`T.text('#panel h2') === 'Home' && T.all('ul.check li').length === 6`, "checklist");
  await shot("home");
  await eq(`T.text('.lead')`, "2 things left to set up.", "open mode (no password) and no speaker test yet");
  await eq(`T.all('ul.check li.done').length`, 4, "done items");
  await eq(
    `T.text('#panel p.mut').startsWith('Today: 3 of 100 language model requests, $0.012 spent')`,
    true,
    "usage line",
  );
  await T(`set('#speaktext', 'Hello')`);
  await T(`submit('#panel form.panel')`);
  await b.until(`T.text('#msg') === 'Sent. Listen for the speaker.'`, "toast");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/speak")!.body, { text: "Hello" });
  await eq(`T.all('ul.check li.done').length`, 5, "speaker test ticked");
  await eq(`T.text('.lead')`, "One thing left to set up.", "lead follows");
  await eq(`T.q('#speaktext').value`, "", "box cleared");
  await T(`clickText('ul.check', 'Set a password')`);
  await b.until(`T.text('#panel h2') === 'Settings'`, "action button navigates");
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
  assert.equal(put.body.seconds, 1200);
  assert.equal(put.body.label, "tea");
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

await step("Agents: text from agents is shown as text", async () => {
  await open("open", "#/agents");
  await b.until(`T.rows().length === 1`, "one event");
  await shot("agents");
  await eq(`T.rows()[0].includes('<img src=x onerror=alert(1)>Build done')`, true, "title shown literally");
  await eq(`T.rows()[0].includes('<script>alert(2)</script> all green')`, true, "text shown literally");
  await eq(`T.all('#panel img, #panel script').length`, 0, "no elements were created from it");
  await eq(`T.text('.badge')`, "Waiting to be read out", "state wording");
});

await step("Tools: turn off, add a web tool, delete", async () => {
  await open("open", "#/tools");
  await b.until(`T.all('h3').some((h) => h.textContent === 'Web tools (1)')`, "web tools heading");
  await shot("tools");
  await T(`clickText('#panel ul.rows > li:nth-child(1)', 'Turn off')`);
  await b.until(`T.all('.badge.bad').some((x) => x.textContent === 'Off')`, "Off badge");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/tools/set_light/enabled")!.body, { enabled: false });
  await T(`click('#panel details summary')`);
  await eq(`T.q('#builtin-tools').open`, true, "built-in tools open");
  await T(`set('#tool-name', 'lamp')`);
  await T(`set('#tool-desc', 'Lamp')`);
  await T(`set('#tool-url', 'https://h/{room}')`);
  await T(`clickText('#panel form.panel', 'Add a value the assistant fills in')`);
  await T(`set('#panel form .row input[aria-label="Value name"]', 'room')`);
  await T(`submit('#panel form.panel')`);
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
  await T(`clickText('#panel ul.rows > li:nth-child(2)', 'Delete')`);
  await b.until(`T.all('h3').some((h) => h.textContent === 'Web tools (1)')`, "deleted");
});

await step("Connections: test, add, turn off, remove", async () => {
  await open("open", "#/connections");
  await b.until(`T.rows('#panel > div > ul.rows > li').length === 1`, "one server");
  await shot("connections");
  await eq(`T.text('.badge.good')`, "Connected", "status badge");
  await T(`set('#conn-name', 'cal')`);
  await T(`set('#conn-url', 'https://cal.local/mcp')`);
  await T(`clickText('#panel form.panel', 'Test connection')`);
  await b.until(
    `T.text('#panel form p[role="status"]') === 'Connected. It offers 2 tools: a_tool, b_tool.'`,
    "test result",
  );
  await T(`submit('#panel form.panel')`);
  await b.until(`T.rows('#panel > div > ul.rows > li').length === 2`, "added");
  assert.deepEqual(mock.log.find((l) => l.method === "POST" && l.path === "/api/connections")!.body, {
    name: "cal",
    url: "https://cal.local/mcp",
    confirm: true,
  });
  await T(`clickText('#panel ul.rows > li:nth-child(2)', 'Turn off')`);
  await b.until(`T.all('.badge.bad').length === 1`, "Off badge");
  await T(`clickText('#panel ul.rows > li:nth-child(2)', 'Remove')`);
  await b.until(`T.rows('#panel > div > ul.rows > li').length === 1`, "removed");
});

await step("Skills: add, edit, turn off, export, import, delete", async () => {
  await open("open", "#/skills");
  await b.until(`T.rows().length === 1`, "one skill");
  await shot("skills");
  await T(`set('#skill-name', 'Morning')`);
  await T(`set('#skill-text', 'Say the weather.')`);
  await T(`set('#skill-tools', 'a, b')`);
  await eq(`T.all('#panel p.mut').some((p) => p.textContent === '16 of 2000 characters')`, true, "character count");
  await T(`submit('#panel form.panel')`);
  await b.until(`T.rows().length === 2`, "skill added");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/skills")!.body, {
    name: "Morning",
    instructions: "Say the weather.",
    tools: ["a", "b"],
    enabled: true,
  });
  await T(`clickText('#panel ul.rows > li:nth-child(2)', 'Edit')`);
  await eq(`T.q('#skill-tools').value`, "a, b", "edit loads the tools");
  await T(`set('#skill-text', 'Say the weather and the news.')`);
  await T(`submit('#panel form.panel')`);
  await b.until(`T.rows()[1].includes('and the news')`, "edit saved");
  await T(`clickText('#panel ul.rows > li:nth-child(2)', 'Turn off')`);
  await b.until(`T.all('.badge').filter((x) => x.textContent === 'Off').length === 1`, "turned off");
  await T(`clickText('#panel', 'Export')`);
  await eq(`JSON.parse(T.q('textarea[aria-label="Exported skills"]').value).format`, "clawdot-skills", "export JSON");
  await T(`set('textarea[aria-label="Skills file to import"]', 'not json')`);
  await T(`clickText('#panel', 'Import as turned off')`);
  await eq(`T.text('#msg')`, "That is not a skills file. Paste the text that Export gave you.", "bad import message");
  await T(
    `set('textarea[aria-label="Skills file to import"]', '{"format":"clawdot-skills","version":1,"skills":[{"name":"Imported","instructions":"x","tools":[]}]}')`,
  );
  await T(`clickText('#panel', 'Import as turned off')`);
  await b.until(`T.rows().length === 3`, "imported");
  await eq(`T.q('textarea[aria-label="Skills file to import"]').value`, "", "import box cleared");
  await T(`clickText('#panel ul.rows > li:nth-child(3)', 'Delete')`);
  await b.until(`T.rows().length === 2`, "deleted");
});

await step("Webhooks: create shows the token once, rotate, delete", async () => {
  await open("open", "#/webhooks");
  await b.until(`T.rows().length === 1`, "one webhook");
  await shot("webhooks");
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
  await open("open", "#/settings");
  await b.until(`!!T.q('#f-device_name')`, "settings forms");
  await shot("settings");
  await eq(`T.text('.soon h3')`, "Restart needed", "restart box");
  await eq(`T.text('.soon p').includes('address this page uses')`, true, "restart list uses plain names");
  await eq(`T.q('#f-voice').disabled`, true, "voice disabled without a speech provider");
  await eq(`T.q('#f-zone').tagName`, "SELECT", "zone is a list");
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
  await T(`set('#f-llm_url', 'http://insecure.example.com')`);
  await b.ev(`T.q('#f-llm_url').form.requestSubmit()`);
  await b.until(
    `T.text('#f-llm_url ~ .errtext') !== null || T.all('.errtext').some((e) => e.textContent.startsWith('The language model address'))`,
    "inline error",
  );
  await eq(`T.all('.errtext').filter((e) => e.textContent).length`, 1, "one error, in its own form");
  await T(`set('#newpw', 'brand new password')`);
  await T(`submit('#pwsec')`);
  await b.until(`T.q('#banner') === null`, "banner gone once a password is set");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/admin/password")!.body, { password: "brand new password" });
  await T(`click('#about summary')`);
  await b.until(`T.text('#notes')?.includes('First release')`, "release notes loaded");
});

await step("a 10-second refresh keeps what you typed", async () => {
  await open("open", "#/reminders");
  await b.until(`T.rows().length === 2`, "rows");
  await T(`set('#rlabel', 'half typed')`);
  (mock.state as any).schedule.push({
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

await step("the page stays inside its own origin and CSP", async () => {
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
    await shot("setup", false);
    await open("open", "#/home");
    await b.until(`T.all('ul.check li').length === 6`, "home");
    await shot("home", false);
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
