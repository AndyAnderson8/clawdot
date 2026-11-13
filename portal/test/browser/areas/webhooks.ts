import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function webhooks(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, shot, step } = c;
  await step("Webhooks: add a webhook in a dialog, token shown once, new token, remove", async () => {
    await open("open", "#/settings/webhooks");
    await b.until(`T.rows().length === 1`, "one webhook");
    await eq(`T.q('#webhookdialog').open`, false, "the add form is not in the way");
    await shot("settings-notifications");
    await eq(`T.q('#snip-claude-code').open`, false, "snippets closed");
    await eq(`T.text('#panel li code')`, mock.url + "/hook/claude-code", "address");
    await T(`click('#addwebhook')`);
    await b.until(`T.q('#webhookdialog').open === true`, "the dialog opens");
    await eq(`T.text('#webhookdialog h3')`, "Add a webhook", "and says what it adds");
    await T(`set('#wname', 'ci')`);
    await T(`set('#wadapter', 'generic')`);
    await T(`set('#wspoken', 'Build server')`);
    await T(`submit('#webhookdialog form')`);
    await b.until(`T.q('#snip-ci')?.open === true && T.q('#snip-ci code.token')`, "token shown");
    await eq(`T.q('#webhookdialog').open`, false, "the dialog closes once the webhook is added");
    await eq(`T.text('#snip-ci code.token')`, "tok-ci-1", "token");
    // What the speaker says a message is from is the user's choice, set here and changed here.
    assert.deepEqual(mock.log.find((l) => l.method === "POST" && l.path === "/api/endpoints")!.body, {
      name: "ci",
      adapter: "generic",
      spoken_name: "Build server",
    });
    await eq(`T.text('#panel li:nth-child(2) .spokenas q')`, "From Build server", "the chosen name is what is spoken");
    await eq(`T.text('#panel li:nth-child(1) .spokenas q')`, "From claude-code", "none chosen: the webhook's own name");
    await T(`clickText('#panel li:nth-child(2) .spokenas', 'Change what it says')`);
    await b.until(`T.q('#spoken-ci').open === true`, "a dialog to change it");
    await T(`set('#spokenname-ci', '<b>Mum</b>')`);
    await T(`submit('#spoken-ci form')`);
    await b.until(`T.text('#spoken-ci .errtext').startsWith('Use up to 40')`, "markup is refused, with the rule");
    await T(`set('#spokenname-ci', 'Garage')`);
    await T(`submit('#spoken-ci form')`);
    await b.until(`T.text('#panel li:nth-child(2) .spokenas q') === 'From Garage'`, "changed");
    assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/endpoints/ci").pop()!.body, {
      spoken_name: "Garage",
    });
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
}
