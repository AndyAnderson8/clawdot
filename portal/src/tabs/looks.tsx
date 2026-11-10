import type { JSX } from "preact";

/** The pictures offered for "what does your device look like". The choice is kept in this browser only. */
export const LOOKS = [
  { id: "puck", name: "Smart speaker" },
  { id: "tower", name: "Tall speaker" },
  { id: "computer", name: "Computer" },
] as const;
export type LookId = (typeof LOOKS)[number]["id"];

const KEY = "clawdot-look";

export function lookPref(): LookId {
  try {
    const v = localStorage.getItem(KEY);
    return LOOKS.find((l) => l.id === v)?.id ?? "puck";
  } catch {
    return "puck";
  }
}

export function setLookPref(id: LookId): void {
  try {
    localStorage.setItem(KEY, id);
  } catch {
    /* private window: the choice lasts until the page is closed */
  }
}

/** A drawing of the device whose light shows what it is doing. `state` is the speaker state from the dashboard. */
export function Look(props: { id: LookId; state?: string }): JSX.Element {
  return (
    <svg class="look" data-state={props.state ?? "idle"} viewBox="0 0 160 120" aria-hidden="true">
      {props.id === "puck" ? (
        <>
          <ellipse cx="80" cy="100" rx="62" ry="9" class="floor" />
          <path d="M20 66v22c0 9 28 16 60 16s60-7 60-16V66Z" class="body" />
          <ellipse cx="80" cy="66" rx="60" ry="16" class="top" />
          <ellipse cx="80" cy="66" rx="50" ry="12" fill="none" class="led" />
          <ellipse cx="80" cy="65" rx="42" ry="8.500" class="cap" />
          <g class="holes">
            <circle cx="62" cy="64" r="1.600" />
            <circle cx="72" cy="66" r="1.600" />
            <circle cx="88" cy="66" r="1.600" />
            <circle cx="98" cy="64" r="1.600" />
          </g>
        </>
      ) : null}
      {props.id === "tower" ? (
        <>
          <ellipse cx="80" cy="108" rx="34" ry="6" class="floor" />
          <path d="M46 28v72c0 5 15 9 34 9s34-4 34-9V28Z" class="body" />
          <ellipse cx="80" cy="28" rx="34" ry="9" class="top" />
          <ellipse cx="80" cy="30" rx="34" ry="9" fill="none" class="led" />
          <g class="holes">
            <circle cx="66" cy="62" r="1.400" />
            <circle cx="80" cy="62" r="1.400" />
            <circle cx="94" cy="62" r="1.400" />
            <circle cx="66" cy="76" r="1.400" />
            <circle cx="80" cy="76" r="1.400" />
            <circle cx="94" cy="76" r="1.400" />
          </g>
        </>
      ) : null}
      {props.id === "computer" ? (
        <>
          <ellipse cx="80" cy="108" rx="48" ry="5" class="floor" />
          <rect x="28" y="16" width="104" height="68" rx="7" class="body" />
          <rect x="35" y="23" width="90" height="54" rx="3" class="cap" />
          <rect x="44" y="64" width="72" height="4" rx="2" class="led solid" />
          <path d="M70 84h20l4 18H66Z" class="top" />
          <rect x="52" y="100" width="56" height="5" rx="2.500" class="body" />
        </>
      ) : null}
    </svg>
  );
}
