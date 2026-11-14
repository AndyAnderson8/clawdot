import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function settingsAbout(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, shot, step } = c;
  await step("About: what it is, version, settings backup and restore, reset all", async () => {
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
    await eq(`T.q('#tologs').getAttribute('href')`, "#/logs/problems", "troubleshooting points to the logs");
    // export what is saved here
    await T(`click('#exportdl')`);
    await b.until(`window.__files.length === 1`, "settings offered as a file");
    await eq(
      `(async () => JSON.parse(await window.__files[0].text()).settings.device_name)()`,
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
    // a file the browser cannot read is explained, and the chooser is empty again so the same file can be chosen
    await b.ev(`(async () => {
    const dt = new DataTransfer();
    const f = new File(['{}'], 'settings.json', { type: 'application/json' });
    Object.defineProperty(f, 'text', { value: () => Promise.reject(new Error('gone')) });
    dt.items.add(f);
    const input = document.querySelector('#importfile');
    input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    await T.settle(); await T.settle();
  })()`);
    await eq(`T.text('#importform .errtext')`, "That file could not be read. Choose it again.", "an unreadable file");
    await eq(`T.q('#importfile').value`, "", "the chooser is reset");
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
}
