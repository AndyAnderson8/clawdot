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
import { STRESS } from "./stress.ts";

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
    wide: () => {
      // A wide stack with extra spacing, so a layout that only fits narrow system fonts (Linux CI) shows itself here.
      const st = document.createElement('style');
      st.id = 'widefont';
      st.textContent = 'html *:not(svg):not(svg *) { font-family: Verdana, "DejaVu Sans", "Arial Black", sans-serif !important; letter-spacing: 0.05em !important; } code, pre, .token { font-family: "Courier New", monospace !important; }';
      if (!document.getElementById('widefont')) document.head.append(st);
    },
    // Things a fixed bar or a clipping box would hide: the focus ring's room, and the last row above the bottom bar.
    hidden: () => {
      const out = [];
      const sel = 'a[href], button, input:not([type=hidden]), select, textarea, summary';
      for (const el of document.querySelectorAll(sel)) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || el.closest('.sr') || getComputedStyle(el).position === 'fixed') continue;
        for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
          if (p.classList.contains('tabs')) break; // a strip meant to scroll sideways
          const cs = getComputedStyle(p);
          if (cs.overflowX === 'visible' && cs.overflowY === 'visible') continue;
          const pr = p.getBoundingClientRect();
          if (r.left - 5 < pr.left - 0.5 || r.right + 5 > pr.right + 0.5) { out.push('focus ring clipped: ' + el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') + ' by ' + p.tagName.toLowerCase() + (p.id ? '#' + p.id : '') + ' (' + Math.round(r.left) + '..' + Math.round(r.right) + ' in ' + Math.round(pr.left) + '..' + Math.round(pr.right) + ')'); break; }
        }
      }
      window.scrollTo(0, document.documentElement.scrollHeight);
      const nav = document.querySelector('#tabs');
      const foot = document.querySelector('#foot');
      if (nav && foot && getComputedStyle(nav).position === 'fixed' && foot.getBoundingClientRect().bottom > nav.getBoundingClientRect().top + 0.5)
        out.push('the bottom bar covers the end of the page (footer bottom ' + Math.round(foot.getBoundingClientRect().bottom) + ' > bar top ' + Math.round(nav.getBoundingClientRect().top) + ')');
      window.scrollTo(0, 0);
      return out.slice(0, 6);
    },
    overflow: () => {
      const vw = document.documentElement.clientWidth;
      const clipped = (el) => { for (let p = el.parentElement; p; p = p.parentElement) { const o = getComputedStyle(p).overflowX; if (o !== 'visible' && p.getBoundingClientRect().right <= vw + 0.5) return true; } return false; };
      const out = [];
      for (const el of document.querySelectorAll('body *')) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.right <= vw + 0.5 || clipped(el)) continue;
        if (getComputedStyle(el).position === 'fixed' && r.right <= vw + 0.5) continue;
        out.push(el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') + (el.className && typeof el.className === 'string' ? '.' + el.className.trim().split(/\\s+/).join('.') : '') + ' right=' + Math.round(r.right) + ' width=' + Math.round(r.width) + ' scrollWidth=' + el.scrollWidth + ' "' + (el.textContent || '').trim().replace(/\\s+/g, ' ').slice(0, 40) + '"');
        if (out.length >= 6) break;
      }
      if (!out.length && document.documentElement.scrollWidth > vw) {
        // Nothing visibly sticks out, so content inside something does: list boxes whose own content is wider than they are.
        const wider = (el) => el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).overflowX === "visible";
        const inner = [...document.querySelectorAll("body *")].filter((el) => wider(el) && ![...el.children].some(wider)).slice(0, 5);
        for (const el of inner) out.push("(content wider than box) " + el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + (typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\\s+/).join(".") : "") + " clientWidth=" + el.clientWidth + " scrollWidth=" + el.scrollWidth);
      }
      return { viewport: vw, scrollWidth: document.documentElement.scrollWidth, offenders: out };
    },
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
  for (const [k, v] of Object.entries(extra)) {
    const have = mock.state[k];
    mock.state[k] =
      v && typeof v === "object" && !Array.isArray(v) && have && typeof have === "object" && !Array.isArray(have)
        ? { ...have, ...v }
        : v;
  }
  await b.goto(mock.url + "/" + hash);
  await b.ev(HELPERS);
}
const T = (expr: string) => b.ev(`T.${expr}`);
const eq = async (expr: string, want: unknown, what: string) => assert.deepEqual(await b.ev(expr), want, what);
const section = async (id: string, label: string) => {
  await b.ev(`location.hash = "#/settings/${id}"`);
  await b.until(`T.text('.tabs a[aria-current="page"]') === ${JSON.stringify(label)}`, `the ${label} section`);
};
const viewport = (width: number) =>
  b.send("Emulation.setDeviceMetricsOverride", { width, height: 800, deviceScaleFactor: 1, mobile: false });
const shot = async (name: string, o: { dark?: boolean; width?: number } = {}) => {
  if (shots)
    await b.screenshot(join(shots, name + ".png"), { width: o.width ?? 390, dark: o.dark ?? false, full: true });
};

// ---------------------------------------------------------------- first run and sign-in
await step("sign-in: the help never tells you to run a command, and says what to do", async () => {
  await open("password");
  await b.until(`T.text('#login h2') === 'Sign in'`, "sign-in screen");
  const help = (await T(`text('#forgothelp')`)) as string;
  assert.equal(
    help,
    "A forgotten password is reset on the speaker itself, with its USB cable or at its console. The guide has the steps for your device.",
  );
  await eq(`T.q('#forgot').tagName`, "DETAILS", "the help is a folded box you can see is clickable");
  await eq(`document.body.innerText.includes('reset-admin')`, false, "no command in the page");
  await eq(`document.body.innerText.toLowerCase().includes('recovery code')`, false, "no recovery code anywhere");
});

await step("reset window: the sign-in page offers a new password only while the device says it is open", async () => {
  await open("password");
  await b.until(`!!T.q('#login')`, "sign-in");
  await eq(`T.q('#windowform') === null`, true, "closed window: nothing offered");
  mock.state["window"] = 240;
  await b.goto(mock.url + "/");
  await b.ev(HELPERS);
  await b.until(`!!T.q('#windowform')`, "open window: the form shows");
  await T(`set('#wpw', 'window password')`);
  await T(`submit('#windowform')`);
  await b.until(`!!T.q('#band')`, "signed in after the reset");
  await eq(`document.body.innerText.toLowerCase().includes('recovery code')`, false, "no code is shown after a reset");
});

await step("setup: mismatched and short passwords are refused, a good one signs in", async () => {
  await open("setup");
  await b.until(`T.text('#setup h2') === 'Set up ClawDot'`, "setup screen");
  await eq(`!!T.q('#hero') && !T.q('#band')`, true, "hero shown, sidebar hidden");
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
  await eq(`document.body.innerText.toLowerCase().includes('recovery')`, false, "no recovery step is asked for");
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
  await b.ev(`location.hash = '#/clock'`); // the tab's first fetch is refused
  await b.until(`!!T.q('#login')`, "sign-in screen after a 401");
  await eq(`T.text('#msg')`, "Please sign in again.", "explanation");
});

// ---------------------------------------------------------------- the shell
await step("shell: four destinations, titles, current page, home link, unknown address", async () => {
  await open("open");
  await b.until(`!!T.q('#band') && !!T.q('#statusline')`, "the dashboard");
  await eq(`T.text('#devname')`, "Kitchen Dot", "the speaker's name is on Home");
  await eq(`T.q('#home').getAttribute('aria-label')`, "ClawDot, go to Home", "the logo is the way home");
  await eq(
    `T.all('#tabs a').map((a) => a.textContent)`,
    ["Home", "Clock", "Media", "Settings"],
    "four destinations, Home first",
  );
  await eq(`T.all('#tabs a[aria-current="page"]').map((a) => a.id)`, ["tab-dashboard"], "home is current first");
  await b.until(`document.title === "Home - ClawDot"`, "the Home title");
  await T(`click('#tab-settings')`);
  await b.until(
    `document.title === "Settings - ClawDot" && T.q('#tab-settings').getAttribute('aria-current') === 'page'`,
    "settings",
  );
  await eq(`document.activeElement.id`, "panel", "focus moves to the page content after a move");
  await T(`click('#home')`);
  await b.until(
    `!!T.q('#statusline') && T.q('#tab-dashboard').getAttribute('aria-current') === 'page'`,
    "the logo goes home",
  );
  await b.ev(`location.hash = '#/nonsense'`);
  await b.until(`!!T.q('#statusline')`, "an unknown address falls back to Home");
  await eq(`T.text('#ver')`, "ClawDot 0.1.0-test", "footer version");
  await T(`click('#newlink')`);
  await b.until(`T.q('#about')?.open === true`, "the release notes open from the footer link");
  await eq(`T.all('#panel [role="tab"], #panel [role="listbox"], [role="combobox"]').length`, 0, "no custom widgets");
});

await step(
  "Home: the device, what needs attention, the numbers, the chart, recent; nothing to manage here",
  async () => {
    await open("open");
    await b.until(
      `T.text('#statusline') === 'Ready.' && T.all('#attention li').length === 1 && (T.text('#statusdetail') ?? '').startsWith('Last heard')`,
      "home",
    );
    await shot("home");
    await shot("home-wide", { width: 1100 });
    await eq(`T.q('#statusblock .sdot').className.includes('live')`, true, "the dot shows the speaker is up");
    await eq(`T.text('#devname')`, "Kitchen Dot", "the speaker's name is the heading");
    await eq(
      `T.text('#statusdetail').startsWith('Last heard')`,
      true,
      "last heard comes from the voice turns in the log",
    );
    await eq(`T.text('#attention h3')`, "One thing needs your attention", "the band says how many");
    await eq(`T.text('#attention li strong')`, "No password", "open mode needs attention");
    await eq(`T.q('#attention li a').getAttribute('href')`, "#/settings/security", "with a fix link");
    await eq(`T.text('#stat-requests .statvalue')`, "3", "requests today");
    await eq(`T.text('#stat-heard .statvalue').endsWith('ago')`, true, "last heard as a number");
    await eq(`T.all('#usage .cbar').length`, 14, "the chart shows fourteen days");
    await b.until(`T.rows('#recent ul.rows > li').length === 6`, "six recent happenings, newest first");
    await eq(`T.all('#recent .badge').length`, 6, "each with an outcome");
    await eq(`T.all('#panel img, #panel script').length`, 0, "no markup from event text");
    // Timers, alarms and the test phrase are not on Home: they have their own places.
    await eq(
      `T.q('#nextup') === null && T.q('#quickadd') === null && T.q('#sayform') === null`,
      true,
      "no reminders or test box",
    );
    await T(`click('#attention li a')`);
    await b.until(
      `T.text('.tabs a[aria-current="page"]') === 'Security'`,
      "the fix link goes to the right settings section",
    );
  },
);

await step("Home: the picture of the device is chosen in a dialog, with a photo of your own", async () => {
  await open("open");
  await b.until(`!!T.q('#statusblock')`, "home");
  await eq(`T.q('#lookdialog').open`, false, "the choices are not in the way");
  await T(`click('#changelook')`);
  await b.until(`T.q('#lookdialog').open === true`, "the dialog opens");
  await eq(`T.all('.lookopt').length`, 3, "three drawings to choose from");
  await T(`click('.lookopt:nth-child(3)')`);
  await eq(`T.q('.lookopt:nth-child(3)').getAttribute('aria-checked')`, "true", "the choice is marked");
  await eq(`localStorage.getItem('clawdot-look')`, "computer", "and remembered by this browser");
  // a photo from this computer: made small, kept here, shown on Home
  await b.ev(`(async () => {
    const c = document.createElement('canvas'); c.width = 1200; c.height = 600;
    const g = c.getContext('2d'); g.fillStyle = '#c33'; g.fillRect(0, 0, 1200, 600);
    const blob = await new Promise((r) => c.toBlob(r, 'image/png'));
    const dt = new DataTransfer(); dt.items.add(new File([blob], 'room.png', { type: 'image/png' }));
    const input = document.getElementById('lookfile'); input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
  await b.until(`!!T.q('#statusblock img.lookphoto')`, "the photo is on Home");
  const size = await b.ev<string>(
    `(() => { const i = T.q('#statusblock img.lookphoto'); return i.naturalWidth + 'x' + i.naturalHeight; })()`,
  );
  assert.equal(size, "480x240", "it was made smaller");
  await eq(`localStorage.getItem('clawdot-look')`, "photo", "and chosen");
  await eq(`T.all('.lookopt').length`, 4, "it is one of the choices now");
  await T(`click('#lookremove')`);
  await b.until(`T.q('#statusblock img.lookphoto') === null`, "removed");
  await eq(`localStorage.getItem('clawdot-look')`, "puck", "back to a drawing");
});

await step("Test the speaker (Settings, Voice): sends the phrase, and says why it cannot, with a fix", async () => {
  await open("open");
  await b.until(`!!T.q('#band')`, "portal");
  await section("voice", "Voice");
  await b.until(`!!T.q('#sayform')`, "the test box");
  await T(`set('#speaktext', 'Hello')`);
  await T(`submit('#sayform')`);
  await b.until(`T.text('#msg') === 'Sent. Listen for the speaker.'`, "toast");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/speak")!.body, { text: "Hello" });
  await eq(`T.q('#speaktext').value`, "", "box cleared");
  // The device says so in its status: the box is disabled before anything is tried.
  await open("open", "#/settings/voice", {
    statusExtra: { speaker: { connected: false }, voice: { configured: false }, muted: false },
  });
  await b.until(`!!T.q('#sayform')`, "the test box");
  await eq(`T.q('#speaktext').disabled && T.q('#saybtn').disabled`, true, "test speak is disabled");
  await eq(`T.text('#sayblock').includes('No voice is set up')`, true, "with the reason");
  await eq(`T.q('#sayfix').getAttribute('href')`, "#/settings/voice", "and the fix link");
  // The device did not say, but refuses the phrase: the reason it gives is shown with its fix.
  await open("open", "#/settings/voice", {
    speakRefusal: { status: 409, reason: "no_speaker", message: "No speaker is connected. Start it." },
  });
  await b.until(`!!T.q('#speaktext') && !T.q('#speaktext').disabled`, "enabled when the status is silent");
  await T(`set('#speaktext', 'Hello')`);
  await T(`submit('#sayform')`);
  await b.until(`!!T.q('#sayblock')`, "the refusal is shown");
  await eq(`T.q('#sayfix').getAttribute('href')`, "#/settings/about", "with its fix link");
});

await step("Home: says why the speaker is not connected, and lists what to fix (REQ-CFG-070)", async () => {
  await open("open", "", {
    statusExtra: { speaker: { connected: false }, voice: { configured: false }, muted: false },
  });
  await b.until(`T.text('#statusline') === 'Waiting for a speaker'`, "no speaker");
  await eq(`T.q('#statusblock .sdot').className.includes('live')`, false, "the dot is not lit");
  await eq(`T.text('#statusdetail').includes('On a Dot this connects by itself')`, true, "says what it means");
  await eq(
    `T.all('#attention li strong').map((x) => x.textContent)`,
    ["No speaker connected", "No voice set up", "No password"],
    "each problem is listed",
  );
  await eq(`T.text('#attention h3')`, "3 things need your attention", "the band counts them");
  await open("open", "", { statusExtra: { muted: true, quiet_hours: { set: true, active: true } } });
  await b.until(`T.text('#statusline') === 'Muted.'`, "muted");
  await eq(
    `T.all('#attention li strong').map((x) => x.textContent).includes('Speaker muted') && T.all('#attention li strong').map((x) => x.textContent).includes('Quiet hours are on')`,
    true,
    "muted and quiet hours are listed",
  );
});

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
  await eq(`T.q('#panel strong b') === null`, true, "no markup from the label");
  await eq(`T.rows()[0].includes('Weekdays at')`, true, "repeat text");
  await eq(`T.text('#clocklist h3').startsWith('Alarms')`, true, "the list has a title and a count");
  await eq(`T.q('#rkind') === null`, true, "no type to choose: this page makes alarms");
  await eq(`T.text('#panel form h3')`, "Add an alarm", "the form says what it makes");
  await T(`set('#rtime', '06:30')`);
  await T(`set('#rlabel', 'gym')`);
  await T(`click('.days input[value="0"]')`);
  await T(`click('.days input[value="2"]')`);
  await T(`submit('#panel form.panel')`);
  await b.until(`T.rows().length === 2`, "alarm added");
  const post = mock.log.filter((l) => l.method === "POST" && l.path === "/api/schedule").pop()!;
  assert.deepEqual(post.body, { kind: "alarm", label: "gym", hour: 6, minute: 30, days: [0, 2] });
  // Timers: the timer is listed here and the alarms are not.
  await b.ev(`location.hash = '#/clock/timers'`);
  await b.until(`T.text('#clocklist h3').startsWith('Timers') && T.rows().length === 1`, "the timer");
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
  await eq(`T.text('#panel form h3')`, "Add a timer", "form left edit mode");
  await T(`clickText('#panel ul.rows > li:nth-child(1)', 'Cancel')`);
  await b.until(
    `T.rows().length === 1 && T.q('#clocklist .empty') !== null`,
    "cancelled, and the empty list says what to do",
  );
  assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/schedule/1"));
  // Reminders: validation.
  await b.ev(`location.hash = '#/clock/reminders'`);
  await b.until(`!!T.q('#rdate')`, "the reminder form");
  await T(`set('#rdate', '')`);
  await T(`submit('#panel form.panel')`);
  await eq(`T.text('#msg')`, "Pick a date and a time for the reminder.", "validation message");
  await eq(`(T.text('#clocklist p.mut') ?? '').length > 0`, true, "zone note present");
  // Stopwatch: used by voice, so this page explains and offers nothing that would not work.
  await b.ev(`location.hash = '#/clock/stopwatch'`);
  await b.until(`!!T.q('#stopwatchcard')`, "the stopwatch page");
  await eq(`T.text('#stopwatchcard').includes('Start the stopwatch')`, true, "says how to use it");
  await eq(`T.q('#panel form') === null && T.q('#panel button') === null`, true, "no controls that do nothing");
});

await step("Media: nothing to play yet, points to Settings; Bluetooth is off and cannot be turned on yet", async () => {
  await open("open", "#/media");
  await b.until(`!!T.q('#btcard')`, "the page");
  await eq(`T.text('#mediacard .blank h3')`, "Nothing to play yet", "says so");
  await eq(`T.q('#mediasettings').getAttribute('href')`, "#/settings/media", "and says where to set it up");
  await eq(`T.q('#btswitch').disabled && !T.q('#btswitch').checked`, true, "Bluetooth is shown off and cannot be used");
  assert.ok(
    mock.log.every((l) => !l.path.includes("bluetooth")),
    "no made-up request",
  );
  await open("open", "#/bluetooth");
  await b.until(`!!T.q('#btcard')`, "the old Bluetooth address lands on Media");
  await open("open", "#/settings/media");
  await b.until(`!!T.q('#mediaproviders')`, "the providers");
  await eq(`T.all('#mediaproviders select').every((x) => x.disabled)`, true, "none can be chosen yet");
  await shot("settings-media");
  await open("open", "#/settings/lights");
  await b.until(`!!T.q('#lightscard')`, "the lights");
  await eq(`T.all('#lightscard svg.ledring').length`, 5, "a preview of the ring for each state");
  await eq(
    `T.all('#lightscard input').every((x) => x.disabled)`,
    true,
    "colours cannot be changed yet, and it says so",
  );
  await shot("settings-lights");
});

await step("Activity: messages from senders are shown as text", async () => {
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
  await eq(`T.text('#eventlist .mut').includes('sender')`, true, "says senders, not agents");
});

await step(
  "Features (a section of Settings): switches turn abilities on and off right away; the old address still works",
  async () => {
    await open("open", "#/settings/features");
    await b.until(`T.all('.features .feature').length === 5`, "five features");
    await shot("features");
    await eq(
      `T.all('.feature h3').map((h) => h.textContent)`,
      ["Remember things", "Say the plan first", "Say what it is doing", "Check for updates", "Practice mode"],
      "in plain words",
    );
    await open("open", "#/features");
    await b.until(
      `T.text('.tabs a[aria-current="page"]') === 'Features'`,
      "the old address opens the Features section of Settings",
    );
    await eq(`T.q('#tab-settings').getAttribute('aria-current')`, "page", "under Settings in the menu");
    await open("open", "#/settings/features");
    await b.until(`T.all('.features .feature').length === 5`, "back on the section");
    await eq(
      `T.all('.feature input.switch').every((i) => i.getAttribute('aria-describedby'))`,
      true,
      "each switch has its explanation",
    );
    const before = await b.ev<boolean>(`T.q('#feat-notes').checked`);
    await T(`click('#feat-notes')`);
    await b.until(`T.q('#feat-notes').checked === ${!before}`, "the switch moved");
    const put = mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!;
    assert.deepEqual(put.body, { notes: !before });
    await eq(`T.q('#card-notes .more').getAttribute('href')`, "#/settings/memory", "memory is managed in Settings");
  },
);

await step("Memory (Settings): add, refuse, forget one, forget all after a confirmation, off state", async () => {
  await open("open", "#/settings/memory");
  await b.until(`T.rows("#notelist > li").length === 1`, "one memory");
  await shot("settings-memory");
  await eq(`T.rows("#notelist > li")[0].includes("my printer is <b>Pixel</b>")`, true, "shown literally");
  await eq(`T.all("#panel #notelist b").length`, 0, "no markup from a memory");
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
  await eq(`T.rows("#notelist > li").length`, 2, "declined confirmation keeps the memory");
  await b.ev(`window.confirm = () => true`);
  await T(`clickText("#notelist > li:nth-child(1)", "Forget")`);
  await b.until(`T.rows("#notelist > li").length === 1`, "one forgotten");
  assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/notes/1"));
  await T(`click("#forgetall")`);
  await b.until(`!!T.q("#noteempty")`, "all forgotten");
  assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/notes"));
  mock.state["notesOn"] = false;
  await b.ev(`location.hash = "#/dashboard"`);
  await section("memory", "Memory");
  await b.until(`!!T.q("#notesoff")`, "the off notice");
  await eq(`T.q("#notesoff a").getAttribute("href")`, "#/settings/features", "it says where to turn it on");
  await eq(`T.q("#note-text").disabled`, true, "adding is disabled while memory is off");
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

await step("Tools: one list, search, how each runs, turn off, add a tool in a dialog, delete", async () => {
  await open("open", "#/settings/tools");
  await b.until(`T.rows('#toolcard ul.rows > li').length >= 2`, "the tools list");
  await shot("settings-tools");
  await eq(`T.q('#tooldialog').open`, false, "the add form is not in the way");
  await eq(`T.q('#tier-set_light').value`, "costly", "a web tool starts at the careful level");
  await eq(
    `[...T.q('#tier-set_light').options].map((o) => o.value)`,
    ["read_only", "reversible", "costly", "destructive"],
    "choices",
  );
  await eq(`T.q('#tier-set_light option[value="costly"]').textContent`, "Asks you first", "in short words");
  await eq(`T.q('#tier-set_light option[value="destructive"]').disabled`, true, "destructive is never offered");
  await b.ev(`window.confirm = () => false`);
  await T(`set('#tier-set_light', 'read_only')`);
  assert.ok(!mock.log.some((l) => l.path === "/api/tools/set_light/tier"), "declining the warning sends nothing");
  await b.ev(`window.confirm = () => true`);
  await T(`set('#tier-set_light', 'reversible')`);
  await b.until(
    `T.q('#tier-set_light').value === 'reversible' && T.text('#toolcard').includes('Changed by you')`,
    "lowered",
  );
  assert.deepEqual(mock.log.find((l) => l.path === "/api/tools/set_light/tier")!.body, { tier: "reversible" });
  await T(`set('#tier-set_light', '')`);
  await b.until(`T.q('#tier-set_light').value === 'costly'`, "back to the usual level");
  assert.deepEqual(mock.log.filter((l) => l.path === "/api/tools/set_light/tier").pop()!.body, { tier: null });
  await T(`click('#on-set_light')`);
  await b.until(`T.q('#on-set_light').checked === false && T.q('.toolrow.off') !== null`, "switched off");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/tools/set_light/enabled")!.body, { enabled: false });
  // search and the source filter
  const total = await b.ev<number>(`T.rows('#toolcard ul.rows > li').length`);
  await T(`set('#toolsearch', 'zzzz-nothing')`);
  await b.until(`T.q('#toolcard .empty') !== null`, "nothing matches");
  await T(`set('#toolsearch', '')`);
  await T(`set('#toolfrom', '@added')`);
  await eq(`T.rows('#toolcard ul.rows > li').length`, 1, "only what was added by hand");
  await T(`set('#toolfrom', '')`);
  await eq(`T.rows('#toolcard ul.rows > li').length`, total, "all again");
  // add a tool from scratch
  await T(`click('#addtool')`);
  await b.until(`T.q('#tooldialog').open === true`, "the dialog opens");
  await T(`set('#tool-name', 'lamp')`);
  await T(`set('#tool-desc', 'Lamp')`);
  await T(`set('#tool-url', 'https://h/{room}')`);
  await T(`click('#adddetail')`);
  await T(`set('#tooldialog input[aria-label="Detail name"]', 'room')`);
  await T(`submit('#tooldialog form')`);
  await b.until(`T.q('#tooldialog').open === false`, "the dialog closes once it is added");
  await b.until(`T.rows('#toolcard ul.rows > li').length === ${total} + 1`, "tool added");
  const post = mock.log.find((l) => l.method === "POST" && l.path === "/api/tools")!;
  assert.deepEqual(post.body, {
    name: "lamp",
    description: "Lamp",
    method: "POST",
    url: "https://h/{room}",
    confirm: true,
    params: { room: { type: "string", required: true } },
  });
  await T(`clickText('#toolcard ul.rows > li:nth-child(2)', 'Delete')`);
  await b.until(`T.rows('#toolcard ul.rows > li').length === ${total}`, "deleted");
});

await step("Sources: connect, choose the tools, add, turn off, remove", async () => {
  const servers = "#sourcecard ul.rows > li";
  await open("open", "#/settings/tools");
  await b.until(`T.rows('${servers}').length === 1`, "one source");
  await eq(`T.text('#sourcecard .badge.good')`, "Connected", "status badge");
  await T(`click('#addsource')`);
  await b.until(`T.q('#sourcedialog').open === true`, "the dialog opens");
  await T(`set('#conn-name', 'cal')`);
  await T(`set('#conn-url', 'https://cal.local/mcp')`);
  await T(`submit('#sourcedialog form')`);
  await b.until(`!!T.q('#sourcetools')`, "its tools are listed");
  await eq(`T.all('#sourcetools .picklist input:checked').length`, 2, "all of them start selected");
  await T(`clickText('#sourcetools', 'Select none')`);
  await eq(`T.text('#sourcetools [role=status]')`, "0 of 2 selected", "and it counts them");
  await T(`click('#sourcetools .picklist input')`);
  await eq(`T.text('#sourcetools [role=status]')`, "1 of 2 selected", "one chosen");
  await T(`submit('#sourcedialog form')`);
  await b.until(`T.rows('${servers}').length === 2`, "added");
  assert.deepEqual(mock.log.find((l) => l.method === "POST" && l.path === "/api/connections")!.body, {
    name: "cal",
    url: "https://cal.local/mcp",
    confirm: true,
  });
  assert.deepEqual(mock.log.filter((l) => l.path === "/api/tools/b_tool/enabled").pop()!.body, { enabled: false });
  await T(`click('${servers}:nth-child(2) input.switch')`);
  await b.until(`T.all('${servers}.off').length === 1`, "turned off");
  await T(`clickText('${servers}:nth-child(2)', 'Remove')`);
  await b.until(`T.rows('${servers}').length === 1`, "removed");
});

await step("Skills: search, select all or none, add, edit in place, folder import, delete", async () => {
  const rows = "#skillcard ul.rows > li";
  await open("open", "#/settings/skills");
  await b.until(`T.rows('${rows}').length === 1`, "one skill");
  await shot("settings-skills");
  await eq(`T.text('#skillcount')`, "1 of 1 turned on", "counted");
  await eq(`T.q('#skilladddialog').open`, false, "the add form is not in the way");
  // add by writing
  await T(`click('#addskill')`);
  await b.until(`T.q('#skilladddialog').open === true`, "the dialog opens");
  await T(`set('#skill-name', 'Morning')`);
  await T(`set('#skill-text', 'Say the weather.')`);
  await T(`set('#skill-tools', 'a, b')`);
  await eq(
    `T.all('#skilladddialog p.mut').some((p) => p.textContent === '16 of 2000 characters')`,
    true,
    "character count",
  );
  await T(`submit('#skilladddialog form')`);
  await b.until(`T.q('#skilladddialog').open === false && T.rows('${rows}').length === 2`, "skill added");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/skills")!.body, {
    name: "Morning",
    instructions: "Say the weather.",
    tools: ["a", "b"],
    enabled: true,
  });
  // edit in place, in a dialog
  await T(`clickText('${rows}:nth-child(2)', 'Edit')`);
  await b.until(`T.q('#skilleditdialog').open === true`, "the editor opens");
  await eq(`T.q('#skilleditdialog #skill-tools').value`, "a, b", "edit loads the tools");
  await T(`set('#skilleditdialog #skill-text', 'Say the weather and the news.')`);
  await T(`submit('#skilleditdialog form')`);
  await b.until(
    `T.q('#skilleditdialog').open === false && T.rows('${rows}')[1].includes('and the news')`,
    "edit saved",
  );
  // search, then turn everything shown off and on
  await T(`set('#skillsearch', 'morning')`);
  await eq(`T.rows('${rows}').length`, 1, "search by name");
  await T(`set('#skillsearch', 'lights')`);
  await eq(`T.rows('${rows}').length`, 1, "search also reads the instructions");
  await T(`set('#skillsearch', '')`);
  await T(`click('#skillnone')`);
  await b.until(`T.text('#skillcount') === '0 of 2 turned on'`, "all turned off");
  await T(`click('#skillall')`);
  await b.until(`T.text('#skillcount') === '2 of 2 turned on'`, "all turned on");
  await T(`click('${rows}:nth-child(2) input.switch')`);
  await b.until(`T.text('#skillcount') === '1 of 2 turned on'`, "one turned off");
  // import from files: the page reads them, lists what it found, and adds the chosen ones turned off
  await T(`click('#addskill')`);
  await T(`clickText('#skilladddialog .seg', 'From a folder or files')`);
  await b.until(`!!T.q('#skillfiles')`, "the file chooser");
  await eq(`T.q('#skilladd').disabled`, true, "nothing to add yet");
  await b.ev(`(() => {
    const dt = new DataTransfer();
    const mk = (text, name, rel) => { const f = new File([text], name); Object.defineProperty(f, 'webkitRelativePath', { value: rel }); return f; };
    dt.items.add(mk('---\\nname: Tidy up\\ndescription: Put things away\\n---\\nAsk which room first.', 'SKILL.md', 'skills/tidy/SKILL.md'));
    dt.items.add(mk('Plain notes', 'notes.md', 'skills/notes.md'));
    dt.items.add(mk('x'.repeat(2100), 'long.md', 'skills/long.md'));
    const input = document.getElementById('skillfiles');
    input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
  await b.until(`!!T.q('#skillfound .picklist')`, "what was found");
  await eq(`T.all('#skillfound .picklist li').length`, 2, "two skills found");
  await eq(`T.text('#skillfound details summary')`, "1 left out", "and one left out, with the reason");
  await T(`click('#skillfound .picklist li:nth-child(2) input')`);
  await eq(`T.text('#skilladd')`, "Add 1 skill", "one chosen");
  await T(`click('#skilladd')`);
  await b.until(`T.q('#skilladddialog').open === false && T.rows('${rows}').length === 3`, "added");
  assert.deepEqual(
    (mock.log.filter((l) => l.path === "/api/skills/import").pop()!.body as { skills: { name: string }[] }).skills.map(
      (s) => s.name,
    ),
    ["Tidy up"],
  );
  await eq(`T.rows('${rows}')[2].includes('Put things away')`, true, "the description came with it");
  // delete from the editor
  await b.ev(`window.confirm = () => true`);
  await T(`clickText('${rows}:nth-child(3)', 'Edit')`);
  await T(`click('#skill-delete')`);
  await b.until(`T.rows('${rows}').length === 2`, "deleted");
});

await step("Notifications: add a sender in a dialog, token shown once, new token, remove", async () => {
  await open("open", "#/settings/notifications");
  await b.until(`T.rows().length === 1`, "one sender");
  await eq(`T.q('#senderdialog').open`, false, "the add form is not in the way");
  await shot("settings-notifications");
  await eq(`T.q('#snip-claude-code').open`, false, "snippets closed");
  await eq(`T.text('#panel li code')`, mock.url + "/hook/claude-code", "address");
  await T(`click('#addsender')`);
  await b.until(`T.q('#senderdialog').open === true`, "the dialog opens");
  await eq(`T.text('#senderdialog h3')`, "Add a sender", "and says what it adds");
  await T(`set('#wname', 'ci')`);
  await T(`set('#wadapter', 'generic')`);
  await T(`submit('#senderdialog form')`);
  await b.until(`T.q('#snip-ci')?.open === true && T.q('#snip-ci code.token')`, "token shown");
  await eq(`T.q('#senderdialog').open`, false, "the dialog closes once the sender is added");
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

await step("Settings: save one group, errors stay in their form, provider presets, password", async () => {
  await open("open", "#/settings/general");
  await b.until(`!!T.q('#f-device_name')`, "settings forms");
  await shot("settings-general");
  await shot("settings-wide", { width: 1100 });
  await eq(`T.text('.soon h4')`, "Restart needed", "restart box");
  await eq(`T.text('.soon p').includes('portal address')`, true, "restart list uses plain names");
  await eq(`T.q('#f-zone').tagName`, "SELECT", "zone is a real select");
  await b.ev(`T.q('#f-device_name').form.requestSubmit()`);
  await b.ev("T.settle()");
  await eq(`T.text('#msg')`, "Nothing to save: you have not changed anything here.", "no change, nothing sent");
  assert.ok(!mock.log.some((l) => l.method === "PUT" && l.path === "/api/settings"));
  await T(`set('#f-device_name', 'Den')`);
  await b.ev(`T.q('#f-device_name').form.requestSubmit()`);
  await b.until(`T.text('#msg') === 'Saved.'`, "saved toast");
  assert.deepEqual(mock.log.find((l) => l.method === "PUT" && l.path === "/api/settings")!.body, {
    device_name: "Den",
  });
  await b.ev(`location.hash = '#/'`);
  await b.until(`T.text('#devname') === 'Den'`, "Home follows the saved name");
  await eq(
    `T.text('.wordmark').replace(/\\s/g, '')`,
    "ClawDt",
    "the brand stays ClawDot, drawn with the ring for the o",
  );
  await section("assistant", "Assistant");
  await shot("settings-assistant");
  await eq(`T.q('#f-voice') === null`, true, "the voice is not a setting of the assistant: it has its own page");
  // a service is picked by name; the address is only asked for when it has to be
  await eq(
    `T.q('#provider').required && T.q('#provider').tagName === 'SELECT'`,
    true,
    "the service is a real dropdown",
  );
  await eq(`T.q('#provider').value`, "custom", "an address that is no known service shows as something else");
  await eq(`!!T.q('#f-llm_url')`, true, "and the address stays editable");
  await T(`set('#provider', 'openai')`);
  await eq(`T.q('#f-llm_url') === null`, true, "a known service hides its address");
  await eq(`T.q('#f-llm_model').value`, "gpt-4o-mini", "the preset fills in a model");
  await eq(`T.q('#f-llm_key_env').value`, "OPENAI_API_KEY", "and the usual key variable");
  await T(`set('#provider', 'custom')`);
  await b.until(`!!T.q('#f-llm_url')`, "something else asks for the address");
  await T(`set('#f-llm_url', 'http://insecure.example.com')`);
  await b.ev(`T.q('#f-llm_url').form.requestSubmit()`);
  await b.until(
    `T.all('.errtext').some((e) => e.textContent.startsWith('The language model address'))`,
    "inline error",
  );
  await eq(`T.all('.errtext').filter((e) => e.textContent).length`, 1, "one error, in its own form");
  await section("security", "Security");
  await eq(`document.body.innerText.toLowerCase().includes('recovery')`, false, "no recovery code in Settings");
  await T(`set('#newpw', 'brand new password')`);
  await T(`submit('#pwsec')`);
  await b.until(`T.q('#banner') === null`, "banner gone once a password is set");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/admin/password")!.body, { password: "brand new password" });
  await section("about", "About");
  await T(`click('#about summary')`);
  await b.until(`T.text('#notes')?.includes('First release')`, "release notes loaded");
});

await step(
  "Settings: the voice and language are chosen on the Voice page; the model comes from the provider",
  async () => {
    // Without a speech provider the language and the voice say so and cannot be changed.
    await open("open", "#/settings/voice");
    await b.until(`!!T.q('#f-speech_language')`, "the language box");
    await eq(`T.q('#f-speech_language').disabled`, true, "disabled without a speech provider");
    await eq(`T.text('#speechprovider').includes('No speech provider is set up')`, true, "and the page says why");
    await open("open", "#/settings/voice", { speechOn: true });
    await b.until(`!!T.q('#f-speech_language') && !T.q('#f-speech_language').disabled`, "the language box");
    await eq(`T.text('#speechprovider dd')`, "speech.example", "the speech provider is shown");
    await eq(`T.q('#f-speech_language').tagName`, "SELECT", "a dropdown, not a code to type");
    await eq(
      `T.all('#f-speech_language option').some((o) => o.textContent === 'English (en)') && T.all('#f-speech_language option').some((o) => o.textContent === 'German (de)')`,
      true,
      "languages are named",
    );
    await eq(
      `T.q('#f-speech_language option').textContent`,
      "Detect it automatically",
      "automatic is the first choice",
    );
    await T(`set('#f-speech_language', 'de')`);
    await b.ev(`T.q('#f-speech_language').form.requestSubmit()`);
    await b.until(`T.text('#msg') === 'Saved.'`, "saved");
    assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
      speech_language: "de",
    });
    // The voice, from the server's list, with a sample.
    await b.until(`T.q('#f-voice')?.tagName === 'SELECT'`, "the voice list");
    await eq(
      `T.all('#f-voice option').map((o) => o.textContent).filter((t) => t !== 'Choose a voice')`,
      ["alloy", "nova", "onyx"],
      "the voices on offer",
    );
    await eq(`T.q('#hearvoice').disabled`, true, "nothing to hear until a voice is chosen");
    await T(`set('#f-voice', 'nova')`);
    await eq(`T.q('#hearvoice').disabled`, true, "a new choice must be saved before it can be heard");
    await eq(`T.text('.voicepick .mut')`, "Save to hear this voice.", "and it says so");
    await b.ev(`T.q('#f-voice').form.requestSubmit()`);
    await b.until(`T.text('#msg') === 'Saved.' && !T.q('#hearvoice').disabled`, "saved, so it can be heard");
    assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
      voice: "nova",
    });
    await T(`click('#hearvoice')`);
    await b.until(`T.text('#msg').startsWith('Playing a sample')`, "the sample is on its way");
    assert.ok(
      String((mock.log.filter((l) => l.path === "/api/speak").pop()!.body as { text: string }).text).startsWith(
        "Hello. This is how I sound.",
      ),
    );
    await eq(`T.q('#sayform') !== null`, true, "the speaker test is on this page, not among the settings");

    // Time zones show their offset, west to east.
    await open("open", "#/settings/general");
    await b.until(`!!T.q('#f-zone')`, "the zone list");
    const zones = await b.ev<string[]>(`T.all('#f-zone option').map((o) => o.textContent)`);
    assert.ok(zones.length > 100 && zones.every((z) => z.startsWith("(UTC")), "every zone starts with its offset");
    const offs = zones.map((z) => {
      const m = /^\(UTC(?:([+-])(\d\d):(\d\d))?\)/.exec(z)!;
      return m[1] ? (m[1] === "-" ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3])) : 0;
    });
    assert.deepEqual(
      [...offs].sort((a, c) => a - c),
      offs,
      "ordered by offset",
    );
    assert.ok(
      zones.some((z) => z.includes("changes with daylight saving")),
      "zones that move say so",
    );

    // The assistant: provider, then its key, then the model as a list the provider supplied.
    await open("open", "#/settings/assistant");
    await b.until(`T.q('#f-llm_model')?.tagName === 'SELECT' && !T.q('#f-llm_model').disabled`, "the model list");
    await eq(
      `T.all('#f-llm_model option').map((o) => o.textContent).filter((t) => t !== 'Choose a model').sort()`,
      ["free/model-c", "model-a", "model-b"],
      "the models the provider offers",
    );
    const labels = await b.ev<string[]>(
      `T.all('#panel form.panel:first-of-type .slabel label').map((l) => l.textContent.replace('Optional', '').trim())`,
    );
    assert.deepEqual(
      labels.slice(0, 4),
      ["Provider", "Address", "API key", "Model"],
      "provider, then its key, then the model",
    );
    await eq(`T.q('#loadmodels') === null`, true, "no button to press to see the models");
    await eq(
      `T.q('#f-llm_fallback_model') === null && T.q('#f-summary_model') === null`,
      true,
      "optional models start out of the way",
    );
    await T(`clickText('.addrow', '+ Add a backup model')`);
    await b.until(`!!T.q('#f-llm_fallback_model')`, "the backup model box appears");
    await eq(`T.text('.optional')`, "Optional", "and is marked optional");
    await b.until(`T.q('#f-llm_fallback_model').tagName === 'SELECT'`, "as a list too");
    await T(`set('#f-llm_fallback_model', 'free/model-c')`);
    await T(`clickText('.sctl', 'Remove')`);
    await b.until(`T.q('#f-llm_fallback_model') === null`, "removed again");
    // The Settings tabs wrap instead of scrolling.
    await eq(`getComputedStyle(T.q('.tabs')).overflowX`, "visible", "no scrollbar on the tab strip");
  },
);

await step("Settings: theme is light unless chosen, remembered, and never follows the system uninvited", async () => {
  await open("open", "#/settings/general");
  await b.until(`!!T.q('#themeform')`, "theme choice");
  await eq(`document.documentElement.dataset.theme`, "light", "light by default");
  // The system prefers dark, and the page stays light.
  await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
  await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(238, 243, 243)", "light on a dark system");
  await eq(
    `T.all('#themeform input[type=radio]').map((r) => r.value + ':' + r.checked)`,
    ["light:true", "dark:false", "system:false"],
    "radios",
  );
  await T(`click('#themeform input[value="system"]')`);
  await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(11, 26, 31)", "match system follows a dark system");
  await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
  await eq(
    `getComputedStyle(document.body).backgroundColor`,
    "rgb(238, 243, 243)",
    "match system follows a light system",
  );
  await T(`click('#themeform input[value="dark"]')`);
  await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(11, 26, 31)", "dark by explicit choice");
  await eq(`localStorage.getItem('clawdot-theme')`, "dark", "remembered by this browser");
  await shot("settings-general-dark", { dark: true });
  await b.goto(mock.url + "/#/settings/general");
  await eq(`document.documentElement.dataset.theme`, "dark", "applied again on load");
  await T(`click('#themeform input[value="light"]')`);
  await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
});

await step("Settings: sections in a tab strip, native dropdowns, no horizontal scroll at 360 px", async () => {
  await open("open", "#/settings");
  await b.until(`T.all('.tabs a').length === 13`, "the section tabs");
  await eq(
    `T.all('.tabs a').map((a) => a.textContent)`,
    [
      "General",
      "Features",
      "Assistant",
      "Voice",
      "Tools",
      "Skills",
      "Media",
      "Lights",
      "Memory",
      "Notifications",
      "Safety",
      "Security",
      "About",
    ],
    "every kind of configuration has a named section",
  );
  await eq(`T.all('.tabs a[aria-current="page"]').map((a) => a.textContent)`, ["General"], "the first is shown");
  await T(`click('.tabs a[href="#/settings/safety"]')`);
  await b.until(`T.text('.tabs a[aria-current="page"]') === 'Safety'`, "a section");
  await viewport(360);
  await eq(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`, true, "no sideways scroll");
  const pages = [
    "#/dashboard",
    "#/clock",
    "#/clock/timers",
    "#/clock/reminders",
    "#/clock/stopwatch",
    "#/media",
    "#/activity",
  ];
  for (const id of [
    "general",
    "features",
    "assistant",
    "voice",
    "tools",
    "skills",
    "media",
    "lights",
    "memory",
    "notifications",
    "safety",
    "security",
    "about",
  ])
    pages.push("#/settings/" + id);
  for (const hash of pages) {
    await b.ev(`location.hash = '${hash}'`);
    await b.ev("T.settle()");
    await b.ev("new Promise((r) => setTimeout(r, 250))");
    // Selects are the platform's own list: real elements, and nothing built to imitate one.
    await eq(`T.all('select').every((e) => e.tagName === 'SELECT')`, true, `real selects on ${hash}`);
    await eq(`T.all('[role=listbox], [role=combobox], [aria-haspopup]').length`, 0, `no custom dropdowns on ${hash}`);
    // A dropdown with nothing chosen must look like one that can be opened, and a disabled field must look disabled.
    await eq(
      `T.all('select:not(:disabled)').every((e) => getComputedStyle(e).backgroundImage.includes('gradient'))`,
      true,
      `every dropdown has an arrow on ${hash}`,
    );
    await eq(
      `T.all('input:disabled:not([type=checkbox]):not([type=radio]), textarea:disabled').every((e) => getComputedStyle(e).borderStyle === 'dashed')`,
      true,
      `disabled fields look disabled on ${hash}`,
    );
  }
  await eq(
    `T.all('input[type=date], input[type=time], input[type=checkbox], input[type=radio], select').every((e) => e.tagName === 'SELECT' || e.tagName === 'INPUT')`,
    true,
    "native inputs",
  );
  await viewport(1100);
});

await step("every page uses the same card pattern: titles, rows, one width for inputs and text areas", async () => {
  const pages = ["#/clock", "#/clock/timers", "#/clock/reminders", "#/clock/stopwatch", "#/media", "#/activity"];
  for (const id of [
    "general",
    "features",
    "assistant",
    "voice",
    "tools",
    "memory",
    "notifications",
    "safety",
    "security",
    "about",
  ])
    pages.push("#/settings/" + id);
  for (const hash of pages) {
    await open("open", hash);
    await b.until(`!!T.q('#panel .card, #panel form.panel')`, `cards on ${hash}`);
    // Nothing sits loose on the page grey except the page title, a lead paragraph and the section tabs.
    const loose = await b.ev<string[]>(`(() => {
      const root = document.querySelector('#panel .page > .stack, #panel .page > .features, #panel .page > .pane, #panel .page');
      return [...(root?.children ?? [])]
        .filter((e) => !e.matches('.card, form.panel, .soon, .stack, .features, header.pagehead, nav.tabs, p.lead, .pane, details'))
        .map((e) => e.tagName.toLowerCase() + (e.className ? '.' + e.className : ''));
    })()`);
    assert.deepEqual(loose, [], `loose blocks on ${hash}`);
    // One look for every text control: inputs and text areas start at the same left edge and share a width.
    const widths = await b.ev<number[]>(
      `[...new Set(T.all('.sctl > input[type=text], .sctl > textarea, .sctl > select').map((e) => Math.round(e.getBoundingClientRect().width)))]`,
    );
    assert.ok(widths.length <= 1, `controls on ${hash} have different widths: ${widths.join(", ")}`);
  }
});

await step("every page fits a 360 px and a 320 px phone with long content and a wide font", async () => {
  const pages = [
    "#/dashboard",
    "#/clock",
    "#/clock/timers",
    "#/clock/reminders",
    "#/clock/stopwatch",
    "#/media",
    "#/activity",
  ];
  for (const id of [
    "general",
    "features",
    "assistant",
    "voice",
    "tools",
    "memory",
    "notifications",
    "safety",
    "security",
    "about",
  ])
    pages.push("#/settings/" + id);
  const bad: string[] = [];
  for (const [content, fixture] of [
    ["stress content", STRESS],
    ["normal content", {}],
  ] as const)
    for (const wide of [false, true]) {
      for (const width of [360, 320]) {
        await open("open", "", fixture);
        await b.until(`!!T.q('#band')`, "the portal");
        if (wide) await b.ev("T.wide()");
        await viewport(width);
        for (const hash of pages) {
          await b.ev(`location.hash = '${hash}'`);
          await b.ev("T.settle()");
          await b.ev("new Promise((r) => setTimeout(r, 400))");
          // open every folded part and every row of a form, so what is inside is measured too
          // open the dialogs too: they are the widest forms
          if (hash.endsWith("/tools")) {
            await T(`click('#addtool')`);
            for (let i = 0; i < 2; i++) await T(`click('#adddetail')`);
          }
          if (hash.endsWith("/skills")) await T(`click('#addskill')`);
          await b.ev("document.querySelectorAll('details').forEach((d) => (d.open = true))");
          await b.ev("new Promise((r) => setTimeout(r, 100))");
          const o = await b.ev<{ viewport: number; scrollWidth: number; offenders: string[] }>("T.overflow()");
          const hidden = await b.ev<string[]>("T.hidden()");
          if (hidden.length)
            bad.push(
              `${hash} at ${width} px, ${content}${wide ? ", wide font" : ""}:\n         ` + hidden.join("\n         "),
            );
          if (o.scrollWidth > o.viewport || o.offenders.length)
            bad.push(
              `${hash} at ${width} px, ${content}${wide ? ", wide font" : ""}: scrollWidth ${o.scrollWidth} > viewport ${o.viewport}\n         ` +
                o.offenders.join("\n         "),
            );
        }
      }
    }
  assert.deepEqual(bad, [], "horizontal overflow:\n  " + bad.join("\n  "));
  await viewport(1100);
});

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
  await b.until(`T.all('.sgroup .head button').some((x) => x.textContent === 'Reset to defaults')`, "the reset button");
  await b.ev(`T.q('#f-agent_max_rounds').form.querySelector('.head button').click()`);
  await b.until(`T.q('#f-agent_max_rounds').value === '8'`, "back to the default");
  assert.deepEqual(mock.log.filter((l) => l.method === "PUT").pop()!.body, { agent_max_rounds: null });
  // safety: quiet hours
  await section("safety", "Safety");
  await eq(`T.q('#f-quiet_start').type`, "time", "quiet hours use a time box");
  await eq(`T.q('#f-dry_run') === null`, true, "practice mode is a switch under Features");
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

await step("About: what it is, version, diagnostics, settings export and import, reset all", async () => {
  await open("open", "#/settings/about");
  await b.until(`!!T.q('#buildinfo')`, "build information");
  await shot("settings-about");
  await eq(`T.text('#aboutver')`, "0.1.0-test", "the version has its own field");
  await eq(
    `document.body.innerText.includes('never contacts anything outside')`,
    false,
    "no claim about staying on the network: the language model is a service",
  );
  await eq(`T.text('#panel .card p').includes('voice assistant')`, true, "says what the project is");
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
  await b.until(`T.text('#msg') === 'Settings imported.'`, "imported and applied");
  assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
    device_name: "Den",
    dry_run: false,
    plan_speech: false,
  });
  // reset everything that was saved
  await T(`click('#resetall')`);
  await b.until(`T.text('#msg') === 'All settings are back to their defaults.'`, "reset");
  const reset = mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body as Record<
    string,
    unknown
  >;
  assert.ok("device_name" in reset && Object.values(reset).every((v) => v === null), "only nulls are sent");
});

const NEWER = {
  available: true,
  latest: {
    version: "0.2.0",
    notes: "Faster wake word.\n<b>not markup</b>",
    page: "https://example.test/release",
    published: "2026-11-10T00:00:00Z",
  },
};

await step("Updates: found, read as text, confirmed, watched, and checked by hand", async () => {
  await open("open", "#/settings/about", { update: { ...NEWER, can_apply: true } });
  await b.until(`!!T.q('#updates') && !!T.q('#updateavail')`, "the updates card");
  await shot("settings-about-update");
  await eq(`T.text('#updateavail')`, "Version 0.2.0 is available.", "what was found");
  await eq(`T.text("#lastchecked")`, "60 min ago", "when it was last looked for");
  await T(`click('#updatenotes summary')`);
  await eq(
    `T.text('#updatenotes pre').includes('<b>not markup</b>') && T.all('#updatenotes b').length === 0`,
    true,
    "release notes are shown as text, never as markup",
  );
  await eq(`T.q('#installupdate').disabled`, false, "it can be installed");
  // Declining the confirmation sends nothing.
  await b.ev(`window.confirm = (m) => { window.__asked = m; return false; }`);
  await T(`click('#installupdate')`);
  assert.ok(!mock.log.some((l) => l.path === "/api/update/apply"), "declined: nothing was sent");
  await eq(
    `window.__asked.includes('Version 0.2.0'.replace('V', 'v')) && window.__asked.includes('signed') && window.__asked.includes('restart')`,
    true,
    "the question says what will happen",
  );
  await b.ev(`window.confirm = () => true`);
  await T(`click('#installupdate')`);
  await b.until(`T.text('#updatephase') === 'Installing.'`, "progress shown");
  assert.deepEqual(mock.log.find((l) => l.path === "/api/update/apply")!.body, { version: "0.2.0" });
  await eq(`T.q('#installupdate').disabled && T.q('#checknow').disabled`, true, "nothing can be pressed twice");
  // The device reports a failure: it is said plainly and the button comes back.
  Object.assign(mock.state["update"] as object, {
    phase: "failed",
    apply_error: "The update was not installed: the downloaded file does not match its signed hash.",
  });
  await b.until(`!!T.q('#applyerr')`, "the failure is shown", 6000);
  await eq(`T.text('#applyerr').includes('does not match its signed hash')`, true, "in plain words");
  await eq(`T.q('#installupdate').disabled`, false, "and it can be tried again");
  // Checking by hand asks the device and shows when.
  Object.assign(mock.state["update"] as object, { phase: "", apply_error: null, last_checked: 1000 });
  await T(`click('#checknow')`);
  await b.until(`T.text('#lastchecked') === 'just now'`, "checked just now");
  assert.ok(mock.log.some((l) => l.method === "POST" && l.path === "/api/update/check"));
});

await step("Updates: a build with no key says why it cannot install, and a failed look says so", async () => {
  await open("open", "#/settings/about", {
    update: {
      ...NEWER,
      can_apply: false,
      cannot_apply_because: "This build has no update key, so an update cannot be checked for authenticity here.",
    },
  });
  await b.until(`!!T.q('#updateavail')`, "found");
  await eq(`T.q('#installupdate').disabled`, true, "nothing to press");
  await eq(`T.text('#cannotupdate').includes('no update key')`, true, "and the reason");
  await eq(
    `T.q('#updatenotes') !== null && T.q('#updates a[href^="https://"]').getAttribute('rel')`,
    "noopener noreferrer",
    "the release page is a safe link",
  );
  await open("open", "#/settings/about", {
    update: { error: "Could not check for updates: the update server answered 404." },
  });
  await b.until(`!!T.q('#updateerr')`, "the error");
  await eq(`T.text('#updateerr')`, "Could not check for updates: the update server answered 404.", "in plain words");
  await eq(`T.q('#uptodate') === null`, true, "it does not claim to be up to date");
  await open("open", "#/settings/about");
  await b.until(`!!T.q('#uptodate')`, "up to date");
  await eq(`T.text('#uptodate')`, "You are up to date.", "said once checked");
});

await step("Updates: Home says a new version exists, and Features can turn the daily check off", async () => {
  await open("open", "", { update: NEWER });
  await b.until(
    `T.all('#attention li strong').some((x) => x.textContent === 'Version 0.2.0 is available')`,
    "the notice on Home",
  );
  await eq(`T.q('#attention li a[href="#/settings/about"]').textContent`, "See what is new", "with where to go");
  await open("open", "#/settings/features");
  await b.until(`!!T.q('#feat-update_check')`, "the switch");
  await eq(`T.q('#feat-update_check').checked`, true, "on by default");
  await T(`click('#feat-update_check')`);
  await b.until(`T.q('#feat-update_check').checked === false`, "turned off");
  assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
    update_check: false,
  });
});

await step("a 10-second refresh keeps what you typed", async () => {
  await open("open", "#/clock/timers");
  await b.until(`T.rows().length === 1`, "rows");
  await T(`set('#rlabel', 'half typed')`);
  (mock.state["schedule"] as unknown[]).push({
    id: 9,
    kind: "timer",
    label: "new one",
    fire_at: Math.floor(Date.now() / 1000) + 60,
    seconds_left: 60,
    repeat: null,
  });
  await b.until(`T.rows().length === 2`, "the refresh brought the new row", 13000);
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
