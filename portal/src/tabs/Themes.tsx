import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { saveFile } from "../parts.tsx";
import { fail } from "../store.ts";
import {
  THEMES,
  clearCustomTheme,
  currentThemeFile,
  customTheme,
  parseTheme,
  setCustomTheme,
  setThemePref,
  themePref,
  type ThemePref,
} from "../theme.ts";

const MAX_FILE_BYTES = 20000;

/** A small picture of a theme: the side panel, the page, a card with a line of text, and the accent. */
function Swatch(props: { colors: [string, string, string, string, string] }): JSX.Element {
  const [rail, page, card, text, accent] = props.colors;
  return (
    <span class="swatch" aria-hidden="true" style={{ background: page }}>
      <span class="swrail" style={{ background: rail }} />
      <span class="swcard" style={{ background: card }}>
        <span class="swline" style={{ background: text }} />
        <span class="swline short" style={{ background: text }} />
        <span class="swdot" style={{ background: accent }} />
      </span>
    </span>
  );
}

/** Choosing how ClawDot looks: a few themes, matching the system, or one of your own from a small file. */
export function Themes(): JSX.Element {
  const [pref, setPref] = useState<ThemePref>(themePref());
  const [mine, setMine] = useState(customTheme());
  const [msg, setMsg] = useState("");
  const [bad, setBad] = useState(false);

  const choose = (p: ThemePref) => {
    setPref(p);
    setThemePref(p);
  };
  const upload = async (ev: Event) => {
    const input = ev.currentTarget as HTMLInputElement;
    const f = input.files?.[0];
    input.value = "";
    if (!f) return;
    if (f.size > MAX_FILE_BYTES) {
      setBad(true);
      setMsg("That file is too big to be a theme.");
      return;
    }
    const t = parseTheme(await f.text());
    if (typeof t === "string") {
      setBad(true);
      setMsg(t);
      return;
    }
    setCustomTheme(t);
    setMine(t);
    setPref("custom");
    setBad(false);
    setMsg("Using " + t.name + ".");
  };
  const remove = () => {
    clearCustomTheme();
    setMine(null);
    setPref("light");
    setBad(false);
    setMsg("Your theme is removed.");
  };
  const template = () => {
    try {
      saveFile("my-clawdot-theme.json", currentThemeFile());
    } catch (x) {
      fail(x);
    }
  };
  const tiles = [
    ...THEMES.map((t) => ({ id: t.id, name: t.name, note: t.note, colors: t.swatch })),
    {
      id: "system" as ThemePref,
      name: "Match system",
      note: "Light or dark, as your device is set.",
      colors: ["#0e2a33", "#eef3f3", "#12262d", "#10232a", "#2ee6c5"] as [string, string, string, string, string],
    },
    ...(mine
      ? [
          {
            id: "custom" as ThemePref,
            name: mine.name,
            note: "Your theme.",
            colors: [
              mine.colors["--color-rail"] ?? "#0e2a33",
              mine.colors["--color-background-body"] ?? "#eef3f3",
              mine.colors["--color-background-surface"] ?? "#ffffff",
              mine.colors["--color-text-primary"] ?? "#10232a",
              mine.colors["--color-live"] ?? "#2ee6c5",
            ] as [string, string, string, string, string],
          },
        ]
      : []),
  ];
  return (
    <section class="card" id="themecard">
      <h3>Themes</h3>
      <p class="mut">
        Choose how ClawDot looks. Light unless you choose otherwise. This is remembered by this browser only.
      </p>
      <div class="themegrid" id="themeform" role="radiogroup" aria-label="Theme">
        {tiles.map((t) => (
          <button
            key={t.id}
            type="button"
            role="radio"
            id={"theme-" + t.id}
            aria-checked={pref === t.id}
            class={"themeopt" + (pref === t.id ? " on" : "")}
            onClick={() => choose(t.id)}
          >
            <Swatch colors={t.colors} />
            <span class="themename">{t.name}</span>
            <span class="mut themenote">{t.note}</span>
          </button>
        ))}
      </div>
      <details class="ownpic" id="owntheme">
        <summary>Make your own theme</summary>
        <div class="ownbody">
          <p class="mut">
            A theme is a small file that lists colours. Download the colours of the theme you are using now, change them
            in a text editor, then upload the file. Only colours can be changed; anything else in the file is refused.
          </p>
          <div class="row">
            <button class="plain small" type="button" id="themetemplate" onClick={template}>
              Download the current colours
            </button>
            <label class="plain small filebtn">
              Upload a theme file
              <input
                id="themefile"
                type="file"
                class="sr"
                accept=".json,application/json"
                onChange={(e) => void upload(e)}
              />
            </label>
            {mine ? (
              <button class="plain small" type="button" id="themeremove" onClick={remove}>
                Remove my theme
              </button>
            ) : null}
          </div>
          {msg ? (
            <p class={bad ? "errtext" : "mut"} role={bad ? "alert" : "status"} id="thememsg">
              {msg}
            </p>
          ) : null}
        </div>
      </details>
    </section>
  );
}
