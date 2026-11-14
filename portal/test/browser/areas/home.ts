import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function shellAndHome(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, section, shot, step } = c;
  // ---------------------------------------------------------------- the shell
  await step("shell: five destinations, titles, current page, home link, unknown address", async () => {
    await open("open");
    await b.until(`!!T.q('#band') && !!T.q('#statusline')`, "the dashboard");
    await eq(`T.text('#devname')`, "Kitchen Dot", "the speaker's name is on Home");
    await eq(`T.q('#home').getAttribute('aria-label')`, "ClawDot, go to Home", "the logo is the way home");
    await b.ev(`new Promise((r) => setTimeout(r, 400))`);
    const asked = mock.log.filter((l) => l.path === "/api/audit").map((l) => l.query);
    assert.ok(
      asked.length > 0 && asked.every((q) => Number(/limit=(\d+)/.exec(q)?.[1]) <= 30),
      "Home asks for a few records",
    );
    await eq(
      `T.all('#tabs a').map((a) => a.textContent)`,
      ["Home", "Clock", "Media", "Logs", "Settings"],
      "five destinations, Home first",
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
      await eq(`T.text('#usage h3')`, "Requests, last 14 days", "and says what it counts");
      await eq(
        `T.text('#usage .chartnote')`,
        "5 requests in two weeks.",
        "yesterday's five count; today's three arrive with the next status",
      );
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
    await eq(`T.q('#ownpic').open`, false, "your own picture is a side option, folded away");
    // an address must be a whole address, and must load as a picture
    await T(`click('#ownpic summary')`);
    await T(`set('#lookurl', 'nope')`);
    await T(`submit('#ownpic form')`);
    await b.until(`T.text('#msg').startsWith('Enter the whole address')`, "a half address is explained");
    await T(`set('#lookurl', location.origin + '/none.png')`);
    await T(`submit('#ownpic form')`);
    await b.until(
      `T.text('#msg') === 'That address did not load as a picture.'`,
      "an address that is not a picture is said so",
    );
    await eq(`localStorage.getItem('clawdot-look')`, "computer", "and nothing changed");
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
    // where the browser will not keep it (a private window), the photo still shows: it is held in the page
    await b.ev(`Storage.prototype.setItem = () => { throw new Error('private window'); }`);
    await b.ev(`(async () => {
    const c = document.createElement('canvas'); c.width = 100; c.height = 50;
    const g = c.getContext('2d'); g.fillStyle = '#33c'; g.fillRect(0, 0, 100, 50);
    const blob = await new Promise((r) => c.toBlob(r, 'image/png'));
    const dt = new DataTransfer(); dt.items.add(new File([blob], 'room2.png', { type: 'image/png' }));
    const input = document.getElementById('lookfile'); input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
    await b.until(
      `(T.q('#statusblock img.lookphoto')?.getAttribute('src') ?? '').startsWith('data:image/')`,
      "the photo shows although it could not be stored",
    );
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
}
