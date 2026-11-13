import { HELPERS } from "../page-helpers.ts";
import type { Ctx } from "../harness.ts";

export async function settingsTheme(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, shot, step } = c;
  await step("Settings: theme is light unless chosen, remembered, and never follows the system uninvited", async () => {
    await open("open", "#/settings/themes");
    await b.until(`!!T.q('#themeform')`, "theme choice");
    await eq(
      `T.all('#themeform [role=radio]').map((r) => r.id)`,
      ["theme-light", "theme-dark", "theme-graphite", "theme-ocean", "theme-contrast", "theme-system"],
      "five themes and matching the system",
    );
    await eq(`document.documentElement.dataset.theme`, "light", "light by default");
    // The system prefers dark, and the page stays light.
    await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
    await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(238, 243, 243)", "light on a dark system");
    await eq(
      `T.all('#themeform [role=radio]').map((r) => r.id.slice(6) + ':' + r.getAttribute('aria-checked')).slice(0, 2).concat(T.q('#theme-system').getAttribute('aria-checked'))`,
      ["light:true", "dark:false", "false"],
      "radios",
    );
    await T(`click('#theme-system')`);
    await eq(
      `getComputedStyle(document.body).backgroundColor`,
      "rgb(11, 26, 31)",
      "match system follows a dark system",
    );
    await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
    await eq(
      `getComputedStyle(document.body).backgroundColor`,
      "rgb(238, 243, 243)",
      "match system follows a light system",
    );
    await T(`click('#theme-dark')`);
    await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(11, 26, 31)", "dark by explicit choice");
    await eq(`localStorage.getItem('clawdot-theme')`, "dark", "remembered by this browser");
    await shot("settings-general-dark", { dark: true });
    await b.goto(mock.url + "/#/settings/themes");
    await eq(`document.documentElement.dataset.theme`, "dark", "applied again on load");
    await T(`click('#theme-light')`);
    await b.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
    // another built-in theme, and one of your own, checked before it is kept
    await T(`click('#theme-graphite')`);
    await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(20, 21, 22)", "graphite");
    await T(`click('#owntheme summary')`);
    const upload = (text: string) =>
      b.ev(`(() => {
      const dt = new DataTransfer(); dt.items.add(new File([${JSON.stringify(text)}], 'mine.json', { type: 'application/json' }));
      const i = document.getElementById('themefile'); i.files = dt.files; i.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await upload("not json");
    await b.until(`T.text('#thememsg').startsWith('That is not a theme file')`, "a file that is not JSON is refused");
    await upload(JSON.stringify({ name: "Bad", colors: { "--color-text-primary": "url(http://x/)" } }));
    await b.until(`T.text('#thememsg').includes('is not a colour')`, "a value that is not a colour is refused");
    await upload(JSON.stringify({ name: "Bad", colors: { background: "#000" } }));
    await b.until(
      `T.text('#thememsg').includes('is not a colour this page can change')`,
      "a name it does not know is refused",
    );
    await eq(`localStorage.getItem('clawdot-theme')`, "graphite", "and the theme stays as it was");
    await upload(
      JSON.stringify({
        name: "Plum",
        dark: true,
        colors: { "--color-background-body": "#220033", "--color-text-primary": "#ffeeff" },
      }),
    );
    await b.until(`T.text('#thememsg') === 'Using Plum.'`, "a good theme is used");
    await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(34, 0, 51)", "its colours apply");
    await eq(`!!T.q('#theme-custom') && T.text('#theme-custom .themename')`, "Plum", "and it has a tile of its own");
    await b.goto(mock.url + "/api/auth");
    await b.goto(mock.url + "/#/settings/themes");
    await eq(`getComputedStyle(document.body).backgroundColor`, "rgb(34, 0, 51)", "applied again on load");
    await b.ev(HELPERS);
    await b.until(`!!T.q('#owntheme')`, "the themes page");
    await T(`click('#owntheme summary')`);
    await T(`click('#themeremove')`);
    await eq(`document.documentElement.dataset.theme`, "light", "removing it goes back to light");
    await eq(`document.adoptedStyleSheets.length`, 0, "and its style sheet is gone");
  });
}
