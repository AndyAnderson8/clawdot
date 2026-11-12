import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function media(c: Ctx): Promise<void> {
  const { b, mock, open, eq, shot, step } = c;
  await step(
    "Media: nothing to play yet, points to Settings; Bluetooth is off and cannot be turned on yet",
    async () => {
      await open("open", "#/media");
      await b.until(`!!T.q('#btcard')`, "the page");
      await eq(`T.text('#mediacard .blank h3')`, "Nothing to play yet", "says so");
      await eq(`T.q('#mediasettings').getAttribute('href')`, "#/settings/media", "and says where to set it up");
      await eq(
        `T.q('#btswitch').disabled && !T.q('#btswitch').checked`,
        true,
        "Bluetooth is shown off and cannot be used",
      );
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
    },
  );
}
