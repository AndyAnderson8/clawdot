// The look of the page. Light unless the person chooses another, and the choice belongs to this browser (localStorage),
// not the device. index.html applies it before the first paint. A theme is a set of colour variables: ClawDot ships a
// few, and a person can upload their own as a small JSON file, which is checked (only known colour names, only colour
// values) before it is kept, so a theme file cannot do anything but change colours.
export type BuiltIn = "light" | "dark" | "graphite" | "contrast" | "ocean";
export type ThemePref = BuiltIn | "system" | "custom";

export interface ThemeInfo {
  id: BuiltIn;
  name: string;
  note: string;
  dark: boolean;
  /** For the preview tile: the side panel, the page, a card, the text and the accent. */
  swatch: [string, string, string, string, string];
}

export const THEMES: ThemeInfo[] = [
  {
    id: "light",
    name: "Light",
    note: "The default.",
    dark: false,
    swatch: ["#0e2a33", "#eef3f3", "#ffffff", "#10232a", "#2ee6c5"],
  },
  {
    id: "dark",
    name: "Dark",
    note: "Easy on the eyes at night.",
    dark: true,
    swatch: ["#07151a", "#0b1a1f", "#12262d", "#e8f3f1", "#2ee6c5"],
  },
  {
    id: "graphite",
    name: "Graphite",
    note: "Dark and neutral.",
    dark: true,
    swatch: ["#0c0d0e", "#141516", "#1d1f21", "#ececec", "#5ad1bd"],
  },
  {
    id: "ocean",
    name: "Ocean",
    note: "Light, with blue.",
    dark: false,
    swatch: ["#0f3d63", "#eaf1f8", "#ffffff", "#0f2233", "#0a6a8a"],
  },
  {
    id: "contrast",
    name: "High contrast",
    note: "Black on white, strong lines.",
    dark: false,
    swatch: ["#000000", "#ffffff", "#ffffff", "#000000", "#00574d"],
  },
];

const KEY = "clawdot-theme";
const CSS_KEY = "clawdot-theme-css";
const META_KEY = "clawdot-theme-meta";

/** The colour variables a theme may set. Anything else in a theme file is refused. */
export const COLOR_NAMES = [
  "--color-background-body",
  "--color-background-surface",
  "--color-background-muted",
  "--color-card-border",
  "--color-text-primary",
  "--color-text-secondary",
  "--color-accent",
  "--color-on-accent",
  "--color-border-emphasized",
  "--color-border-control",
  "--color-text-error",
  "--color-text-warning",
  "--color-text-success",
  "--color-warning-muted",
  "--color-alert-line",
  "--color-live",
  "--color-live-muted",
  "--color-mint",
  "--color-chart",
  "--color-rail",
  "--color-rail-text",
  "--color-rail-strong",
  "--color-rail-active",
  "--color-rail-line",
] as const;

export interface CustomTheme {
  name: string;
  dark: boolean;
  colors: Record<string, string>;
}

export function isThemePref(v: unknown): v is ThemePref {
  return v === "system" || v === "custom" || THEMES.some((t) => t.id === v);
}

export function themePref(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (isThemePref(v) && (v !== "custom" || customTheme())) return v;
  } catch {
    /* storage may be blocked */
  }
  return "light";
}

export function setThemePref(pref: ThemePref): void {
  try {
    localStorage.setItem(KEY, pref);
  } catch {
    /* the choice then lasts until the page closes */
  }
  document.documentElement.dataset["theme"] = pref;
}

const COLOR_VALUE = /^(#[0-9a-fA-F]{3,8}|rgba?\(\s*[0-9.]+%?(\s*[,\s/]\s*[0-9.]+%?){2,3}\s*\))$/;

/** A theme file, checked: its name, whether it is dark, and colours with known names and colour values only. */
export function parseTheme(text: string): CustomTheme | string {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return "That is not a theme file. It should be JSON, like the template you can download.";
  }
  if (typeof v !== "object" || v === null) return "That is not a theme file.";
  const o = v as { name?: unknown; dark?: unknown; colors?: unknown };
  const name = typeof o.name === "string" ? o.name.trim().slice(0, 40) : "";
  if (!name) return 'Give the theme a name, in a "name" field.';
  if (typeof o.colors !== "object" || o.colors === null || Array.isArray(o.colors))
    return 'The theme needs a "colors" list.';
  const colors: Record<string, string> = {};
  for (const [k, c] of Object.entries(o.colors as Record<string, unknown>)) {
    if (!(COLOR_NAMES as readonly string[]).includes(k))
      return '"' + k.slice(0, 40) + '" is not a colour this page can change.';
    if (typeof c !== "string" || !COLOR_VALUE.test(c.trim()))
      return "The value for " + k + " is not a colour. Use a form like #1a2b3c.";
    colors[k] = c.trim();
  }
  if (!Object.keys(colors).length) return "The theme has no colours in it.";
  return { name, dark: o.dark === true, colors };
}

/** The style that applies a theme, built only from names and values that passed the check. */
export function themeCss(t: CustomTheme): string {
  const body = Object.entries(t.colors)
    .map(([k, c]) => k + ":" + c + ";")
    .join("");
  return ':root[data-theme="custom"]{color-scheme:' + (t.dark ? "dark" : "light") + ";" + body + "}";
}

export function customTheme(): CustomTheme | null {
  try {
    const m = localStorage.getItem(META_KEY);
    if (!m) return null;
    const t = parseTheme(m);
    return typeof t === "string" ? null : t;
  } catch {
    return null;
  }
}

/** The stylesheet of the uploaded theme (index.html applies it before the first paint and marks it). */
type ThemeSheet = CSSStyleSheet & { customTheme?: boolean };

function removeThemeSheet(): void {
  document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => !(s as ThemeSheet).customTheme);
}

/** Keep an uploaded theme and use it now. */
export function setCustomTheme(t: CustomTheme): void {
  try {
    localStorage.setItem(META_KEY, JSON.stringify(t));
    localStorage.setItem(CSS_KEY, themeCss(t));
  } catch {
    /* it lasts until the page closes */
  }
  removeThemeSheet();
  // A constructed stylesheet, not a <style> element: the page's content-security policy refuses inline styles that
  // are not its own, and this CSS comes from a file the person uploaded.
  const sheet = new CSSStyleSheet() as ThemeSheet;
  sheet.replaceSync(themeCss(t));
  sheet.customTheme = true;
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  setThemePref("custom");
}

/** Forget the uploaded theme. */
export function clearCustomTheme(): void {
  try {
    localStorage.removeItem(META_KEY);
    localStorage.removeItem(CSS_KEY);
  } catch {
    /* nothing to do */
  }
  removeThemeSheet();
  if (document.documentElement.dataset["theme"] === "custom") setThemePref("light");
}

/** The colours the page is showing now, as a theme file to start from. */
export function currentThemeFile(name = "My theme"): string {
  const cs = getComputedStyle(document.documentElement);
  const colors: Record<string, string> = {};
  for (const k of COLOR_NAMES) {
    const v = cs.getPropertyValue(k).trim();
    if (v) colors[k] = v;
  }
  const dark = cs.colorScheme.includes("dark");
  return JSON.stringify({ name, dark, colors }, null, 2);
}
