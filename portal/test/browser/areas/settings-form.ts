import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function settingsForm(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, section, shot, step } = c;
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
    await eq(`T.q('#provider').value`, "custom", "an address that is no known service shows as custom");
    await eq(`!!T.q('#f-llm_url')`, true, "and the address stays editable");
    await T(`set('#provider', 'openai')`);
    await eq(`T.q('#f-llm_url') === null`, true, "a known service hides its address");
    await eq(`T.q('#f-llm_model').value`, "gpt-4o-mini", "the preset fills in a model");
    await eq(`T.q('#f-llm_key_env') === null`, true, "and the key variable, which is only for custom setups");
    await eq(`!!T.q('#apikey') || !!T.q('#keyhint')`, true, "a known service asks for its key instead");
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
    // Now there is a password, changing it asks for the current one, and a wrong one changes nothing.
    await section("security", "Security");
    await b.until(`!!T.q('#curpw')`, "the current password is asked for");
    await T(`set('#newpw', 'another password 2')`);
    await T(`submit('#pwsec')`);
    await b.until(`T.text('#pwerr').startsWith('Type your current password')`, "it must be typed");
    await T(`set('#curpw', 'not the password')`);
    await T(`submit('#pwsec')`);
    await b.until(`T.text('#pwerr').startsWith('That is not the current password')`, "a wrong one is refused");
    await eq(`T.q('#newpw').value`, "another password 2", "what was typed stays");
    await eq(`!!T.q('#curpw')`, true, "and the page is still signed in");
    await T(`set('#curpw', 'brand new password')`);
    await T(`submit('#pwsec')`);
    await b.until(`T.text('#msg') === 'Password saved.'`, "the right one changes it");
    assert.deepEqual(mock.log.filter((l) => l.path === "/api/admin/password").pop()!.body, {
      password: "another password 2",
      current: "brand new password",
    });
    await section("about", "About");
    await T(`click('#about summary')`);
    await b.until(`T.text('#notes')?.includes('First release')`, "release notes loaded");
  });
}
