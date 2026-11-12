import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

const NEWER = {
  available: true,
  latest: {
    version: "0.2.0",
    notes: "Faster wake word.\n<b>not markup</b>",
    page: "https://example.test/release",
    published: "2026-11-10T00:00:00Z",
  },
};

export async function updates(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, shot, step } = c;
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
    await b.ev(`window.__answer = (m) => { window.__asked = m; return false; }`);
    await T(`click('#installupdate')`);
    assert.ok(!mock.log.some((l) => l.path === "/api/update/apply"), "declined: nothing was sent");
    await eq(
      `window.__asked.includes('Version 0.2.0'.replace('V', 'v')) && window.__asked.includes('signed') && window.__asked.includes('restart')`,
      true,
      "the question says what will happen",
    );
    await b.ev(`window.__answer = () => true`);
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
}
