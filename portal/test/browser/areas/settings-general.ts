import { HELPERS } from "../page-helpers.ts";
import type { Ctx } from "../harness.ts";

export async function settingsGeneral(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, step } = c;
  await step(
    "Settings: General says what the device is; settings it cannot use are hidden until asked for",
    async () => {
      await open("open", "#/settings/general");
      await b.until(`!!T.q('#hardwarecard dd')`, "the device card");
      await eq(
        `T.all('#panel .card h3').map((h) => h.textContent).slice(0, 2)`,
        ["Device", "This device"],
        "it is the second card of General",
      );
      await eq(`T.text('#hwboard')`, "Test Speaker", "the configured label names the device");
      await eq(`T.q('#hwunknown') === null`, true, "a configured device has no 'no preset' note");
      await eq(
        `T.all('#hwparts li')[0].querySelector('.badge').textContent`,
        "Set in configuration",
        "a part the configuration declared says so",
      );
      await eq(
        `T.all('#hwparts li')[2].querySelector('.badge').textContent`,
        "Found",
        "a part that was looked at says found",
      );
      await eq(`T.text('#hardwarecard').includes('486 MB')`, true, "its memory");
      await eq(
        `T.all('#hwparts li').map((l) => l.textContent.slice(0, 5) + ' ' + (l.querySelector('.badge')?.textContent ?? '')).length`,
        5,
        "each part is listed",
      );
      await eq(
        `T.all('.tabs a').some((a) => a.textContent === 'Lights')`,
        true,
        "a device with lights lists the Lights page",
      );
      // a device with no configured preset is called by the model it gives itself, and the page says no preset is set
      mock.state["hwNoLabel"] = true;
      await b.goto(mock.url + "/api/auth");
      await b.goto(mock.url + "/#/settings/general");
      await b.ev(HELPERS);
      await b.until(`!!T.q('#hardwarecard dd')`, "the device card");
      await eq(`T.text('#hwboard')`, "Test Model 9", "no label: the model string");
      await eq(`!!T.q('#hwunknown')`, true, "no preset: it says so");
      mock.state["hwNoLabel"] = false;
      // a device with no lights does not list them, unless asked
      mock.state["hwLeds"] = false;
      await b.goto(mock.url + "/api/auth");
      await b.goto(mock.url + "/#/settings/general");
      await b.ev(HELPERS);
      await b.until(`!!T.q('#hardwarecard dd')`, "the device card");
      await b.until(`!T.all('.tabs a').some((a) => a.textContent === 'Lights')`, "no Lights page without lights");
      await eq(
        `T.all('.tabs a').some((a) => a.textContent === 'Zigbee' || a.textContent === 'Intercom')`,
        false,
        "and nothing that is only planned",
      );
      await T(`click('#showall')`);
      await b.until(
        `T.all('.tabs a').some((a) => a.textContent === 'Lights') && T.all('.tabs a').some((a) => a.textContent === 'Intercom')`,
        "everything, once asked",
      );
      await T(`click('.tabs a[href="#/settings/lights"]')`);
      await b.until(`!!T.q('#lacking')`, "the page says why it cannot be used");
      await eq(`T.text('#lacking').includes('No light ring')`, true, "in words");
      await T(`click('.tabs a[href="#/settings/intercom"]')`);
      await b.until(`!!T.q('#planned')`, "a planned page says it is not built");
      await T(`click('#showall')`);
      await b.ev(`localStorage.removeItem('clawdot-show-unavailable')`);
      mock.state["hwLeds"] = true;
    },
  );

  await step("Settings: other speakers on the network are listed, or the page says it is not looking", async () => {
    await open("open", "#/settings/speakers");
    await b.until(`!!T.q('#speakersoff')`, "not turned on: it says so, and how it is turned on");
    await eq(`T.q('#peerlist') === null`, true, "and lists nothing made up");
    await open("open", "#/settings/speakers", {
      hive: [{ id: "8e003daaa50dbc7b", name: "<b>Bedroom</b>", address: "192.0.2.7", port: 3335, last_seen_secs: 70 }],
    });
    await b.until(`T.rows('#peerlist > li').length === 1`, "one other speaker");
    await eq(`T.text('#selfname')`, "Kitchen Dot", "this speaker is named");
    await eq(
      `T.rows('#peerlist > li')[0].includes('<b>Bedroom</b>') && T.all('#peerlist b').length === 0`,
      true,
      "a name from the network is shown as text",
    );
    await eq(
      `T.rows('#peerlist > li')[0].includes('192.0.2.7') && T.rows('#peerlist > li')[0].includes('ago')`,
      true,
      "with its address and when it was last seen",
    );
    await eq(`T.q('#peerlist a').getAttribute('href')`, "http://192.0.2.7:3335/", "and a way to its own page");
  });
}
