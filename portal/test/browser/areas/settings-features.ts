import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function features(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, shot, step } = c;
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
}
