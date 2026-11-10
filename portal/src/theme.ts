// The look of the page: light unless the person chooses dark or "match system" in Settings. The choice belongs to
// this browser (localStorage), not to the device, and the page works without storage. index.html applies it before
// the first paint.
export type ThemePref = "light" | "dark" | "system";
const KEY = "clawdot-theme";

export function isThemePref(v: unknown): v is ThemePref {
  return v === "light" || v === "dark" || v === "system";
}

export function themePref(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (isThemePref(v)) return v;
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
