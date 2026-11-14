import type { JSX } from "preact";

/** The pictures offered for "what does your device look like". The choice is kept in this browser only. */
export const LOOKS = [
  { id: "puck", name: "Smart speaker" },
  { id: "tower", name: "Tall speaker" },
  { id: "computer", name: "Computer" },
] as const;
/** A drawing, or the person's own photo. */
export type LookId = (typeof LOOKS)[number]["id"] | "photo";

const KEY = "clawdot-look";
const PHOTO = "clawdot-look-photo";
/** The longest side of a saved photo, in pixels. It is stored in this browser, so it is kept small. */
export const PHOTO_SIDE = 480;

export function photoPref(): string {
  try {
    const v = localStorage.getItem(PHOTO) ?? "";
    return v.startsWith("data:image/") || /^https?:\/\/\S+$/.test(v) ? v : "";
  } catch {
    return "";
  }
}

export function lookPref(): LookId {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "photo" && photoPref()) return "photo";
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

/** Keeps the person's own picture (a small photo inside this browser, or an image address) and chooses it. An empty string removes it. */
export function setPhoto(dataUrl: string): void {
  try {
    if (dataUrl) {
      localStorage.setItem(PHOTO, dataUrl);
      localStorage.setItem(KEY, "photo");
    } else {
      localStorage.removeItem(PHOTO);
      if (localStorage.getItem(KEY) === "photo") localStorage.setItem(KEY, "puck");
    }
  } catch {
    /* private window or storage full: the choice lasts until the page is closed */
  }
}

/** Reads a chosen image file and returns it scaled down to a small JPEG, or says why it cannot. */
export function readPhoto(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    if (!file.type.startsWith("image/")) return reject(new Error("That file is not a picture."));
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("The picture could not be read."));
    reader.onload = () => {
      const img = new Image();
      img.onerror = () => reject(new Error("That picture could not be opened. Try a JPEG or PNG."));
      img.onload = () => {
        const k = Math.min(1, PHOTO_SIDE / Math.max(img.width, img.height));
        const c = document.createElement("canvas");
        c.width = Math.max(1, Math.round(img.width * k));
        c.height = Math.max(1, Math.round(img.height * k));
        const g = c.getContext("2d");
        if (!g) return reject(new Error("The picture could not be prepared."));
        g.drawImage(img, 0, 0, c.width, c.height);
        resolve(c.toDataURL("image/jpeg", 0.85));
      };
      img.src = typeof reader.result === "string" ? reader.result : "";
    };
    reader.readAsDataURL(file);
  });
}

/** A drawing of the device whose light shows what it is doing. `state` is the speaker state from the dashboard. */
export function Look(props: { id: LookId; state?: string; photo?: string }): JSX.Element {
  if (props.id === "photo")
    return <img class="lookphoto" src={props.photo ?? photoPref()} alt="" data-state={props.state ?? "idle"} />;
  return (
    <svg class="look" data-state={props.state ?? "idle"} viewBox="0 0 160 120" aria-hidden="true">
      {props.id === "puck" ? (
        <>
          <ellipse cx="80" cy="102" rx="62" ry="8" class="floor" />
          <path d="M20 64v24c0 9 28 16 60 16s60-7 60-16V64Z" class="body" />
          <ellipse cx="80" cy="64" rx="60" ry="16" class="top" />
          <ellipse cx="80" cy="64" rx="53" ry="12.500" fill="none" class="led" />
          <ellipse cx="80" cy="64" rx="45" ry="9.500" class="cap" />
          <g class="holes">
            <ellipse cx="53" cy="64" rx="3.200" ry="1.400" />
            <ellipse cx="68" cy="64" rx="3.200" ry="1.400" />
            <ellipse cx="92" cy="64" rx="3.200" ry="1.400" />
            <ellipse cx="107" cy="64" rx="3.200" ry="1.400" />
          </g>
        </>
      ) : null}
      {props.id === "tower" ? (
        <>
          <ellipse cx="80" cy="108" rx="34" ry="6" class="floor" />
          <path d="M46 28v72c0 5 15 9 34 9s34-4 34-9V28Z" class="body" />
          <ellipse cx="80" cy="28" rx="34" ry="9" class="top" />
          <ellipse cx="80" cy="28" rx="30" ry="7" fill="none" class="led" />
          <g class="holes">
            {[50, 62, 74, 86].flatMap((y) =>
              [64, 80, 96].map((x) => <circle key={x + "-" + y} cx={x} cy={y} r="1.500" />),
            )}
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
