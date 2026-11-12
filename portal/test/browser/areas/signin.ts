import assert from "node:assert/strict";
import { PASSWORD } from "../mock-api.ts";
import { HELPERS } from "../page-helpers.ts";
import type { Ctx } from "../harness.ts";

export async function signIn(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, section, shot, step } = c;
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
    await eq(
      `document.body.innerText.toLowerCase().includes('recovery code')`,
      false,
      "no code is shown after a reset",
    );
  });

  await step("setup: mismatched and short passwords are refused, a good one signs in", async () => {
    await open("setup");
    await b.until(`T.text('#setup h2') === 'Set up ClawDot'`, "setup screen");
    await eq(`!!T.q('#hero') && !T.q('#band')`, true, "hero shown, sidebar hidden");
    await shot("setup");
    // what is typed can be shown, and the page says how the two boxes stand while you type
    await eq(`T.q('#pw1').type`, "password", "hidden to begin with");
    await T(`click('#pw1')`);
    await T(`clickText('.authfield:nth-of-type(1)', 'Show')`);
    await eq(`T.q('#pw1').type`, "text", "shown on request");
    await T(`clickText('.authfield:nth-of-type(1)', 'Hide')`);
    await eq(`T.q('#pw1').type`, "password", "and hidden again");
    await T(`set('#pw1', 'abc')`);
    await eq(`T.text('#pwhint')`, "Use 8 or more characters.", "too short, and it says so");
    await T(`set('#pw1', 'longenough1')`);
    await eq(`T.text('#pwhint')`, "Type it again to be sure.", "long enough");
    await T(`set('#pw2', 'longenough')`);
    await eq(`T.text('#pwhint')`, "They do not match yet.", "not the same yet");
    await T(`set('#pw2', 'longenough1')`);
    await eq(`T.text('#pwhint')`, "They match.", "the same");
    await T(`set('#pw1', '')`);
    await T(`set('#pw2', '')`);
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
}
