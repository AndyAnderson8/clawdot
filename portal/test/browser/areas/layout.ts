import assert from "node:assert/strict";
import { STRESS } from "../stress.ts";
import type { Ctx } from "../harness.ts";

export async function layout(c: Ctx): Promise<void> {
  const { b, open, T, eq, viewport, step } = c;
  await step("Settings: sections in a tab strip, native dropdowns, no horizontal scroll at 360 px", async () => {
    await open("open", "#/settings");
    await b.until(`T.all('.tabs a').length === 14`, "the section tabs");
    await eq(
      `T.all('.tabs a').map((a) => a.textContent)`,
      [
        "General",
        "Themes",
        "Features",
        "Assistant",
        "Voice",
        "Tools",
        "Skills",
        "Memory",
        "Media",
        "Lights",
        "Webhooks",
        "Speakers",
        "Security",
        "About",
      ],
      "every kind of configuration has a named section",
    );
    await eq(
      `T.all('.tabs .navhead').map((a) => a.textContent)`,
      ["Basics", "Assistant", "Integrations", "System"],
      "in groups",
    );
    await eq(`T.all('.tabs a[aria-current="page"]').map((a) => a.textContent)`, ["General"], "the first is shown");
    await T(`click('.tabs a[href="#/settings/security"]')`);
    await b.until(`T.text('.tabs a[aria-current="page"]') === 'Security'`, "a section");
    await viewport(360);
    await eq(
      `document.documentElement.scrollWidth <= document.documentElement.clientWidth`,
      true,
      "no sideways scroll",
    );
    const pages = [
      "#/dashboard",
      "#/clock",
      "#/clock/timers",
      "#/clock/reminders",
      "#/clock/stopwatch",
      "#/media",
      "#/logs/activity",
      "#/logs/problems",
      "#/logs/conversations",
    ];
    for (const id of [
      "general",
      "themes",
      "features",
      "assistant",
      "voice",
      "tools",
      "skills",
      "media",
      "lights",
      "memory",
      "webhooks",
      "speakers",
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
    const pages = [
      "#/clock",
      "#/clock/timers",
      "#/clock/reminders",
      "#/clock/stopwatch",
      "#/media",
      "#/logs/activity",
      "#/logs/problems",
      "#/logs/conversations",
    ];
    for (const id of ["general", "features", "assistant", "voice", "tools", "memory", "webhooks", "security", "about"])
      pages.push("#/settings/" + id);
    for (const hash of pages) {
      await open("open", hash);
      await b.until(`!!T.q('#panel .card, #panel form.panel')`, `cards on ${hash}`);
      // Nothing sits loose on the page grey except the page title, a lead paragraph and the section tabs.
      const loose = await b.ev<string[]>(`(() => {
      const root = document.querySelector('#panel .page > .settings > .pane, #panel .page > .stack, #panel .page > .features, #panel .page > .pane, #panel .page');
      return [...(root?.children ?? [])]
        .filter((e) => !e.matches('.card, form.panel, .soon, .stack, .features, header.pagehead, nav.tabs, p.lead, .pane, .settings, details'))
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
      "#/logs/activity",
      "#/logs/problems",
      "#/logs/conversations",
    ];
    for (const id of ["general", "features", "assistant", "voice", "tools", "memory", "webhooks", "security", "about"])
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
                `${hash} at ${width} px, ${content}${wide ? ", wide font" : ""}:\n         ` +
                  hidden.join("\n         "),
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
}

export async function crossCutting(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, step } = c;
  await step("a 10-second refresh keeps what you typed", async () => {
    await open("open", "#/clock/timers");
    await b.until(`T.rows().length === 1`, "rows");
    await T(`click('#addclock')`);
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
}

export async function lightScreenshots(c: Ctx): Promise<void> {
  const { b, shots, open, shot, step } = c;
  if (shots)
    await step("light-mode screenshots", async () => {
      await open("setup");
      await b.until(`!!T.q('#setup')`, "setup");
      await shot("setup");
      await open("open", "#/dashboard");
      await b.until(`!!T.q('#statusline')`, "dashboard");
      await shot("dashboard");
    });
}
