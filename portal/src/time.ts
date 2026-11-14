// Time is always shown in the device's zone, never the browser's. Pure functions; the zone is passed in.
import type { Repeat } from "./types.ts";

export const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

function formatter(locale: string | undefined, zone: string, o: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  try {
    return new Intl.DateTimeFormat(locale, { timeZone: zone, ...o });
  } catch {
    return new Intl.DateTimeFormat(locale, o);
  } // an unknown zone name falls back to the browser's
}

export function fmt(ts: number, zone: string, o: Intl.DateTimeFormatOptions): string {
  return formatter(undefined, zone, o).format(new Date(ts * 1000));
}

/** `{ date: "2026-11-09", time: "07:30" }` in the zone. */
export function parts(ts: number, zone: string): { date: string; time: string } {
  const m: Record<string, string> = {};
  formatter("en-CA", zone, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  })
    .formatToParts(new Date(ts * 1000))
    .forEach((p) => {
      m[p.type] = p.value;
    });
  return { date: `${m["year"]}-${m["month"]}-${m["day"]}`, time: `${m["hour"]}:${m["minute"]}` };
}

export function span(secs: number): string {
  if (secs < 90) return secs + " s";
  if (secs < 5400) return Math.round(secs / 60) + " min";
  if (secs < 129600) {
    const hh = Math.floor(secs / 3600);
    const mm = Math.round((secs % 3600) / 60);
    return hh + " h" + (mm ? " " + mm + " min" : "");
  }
  return Math.round(secs / 86400) + " days";
}

export function ago(ts: number, now: number): string {
  const s = Math.max(0, now - ts);
  return s < 60 ? "just now" : span(s) + " ago";
}

/** The calendar day after `date` ("2026-11-01" gives "2026-11-02"); a day is not always 86400 seconds long. */
function nextDay(date: string): string {
  const [y = 0, m = 1, d = 1] = date.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d + 1)).toISOString().slice(0, 10);
}

export function whenText(ts: number, now: number, zone: string): string {
  const a = parts(ts, zone);
  const b = parts(now, zone);
  const tom = nextDay(b.date);
  const day =
    a.date === b.date
      ? "Today"
      : a.date === tom
        ? "Tomorrow"
        : fmt(ts, zone, { weekday: "short", month: "short", day: "numeric" });
  return day + " at " + fmt(ts, zone, { hour: "numeric", minute: "2-digit" });
}

export function repeatText(r: Repeat): string {
  const d = r.days.join(",");
  if (r.every_day) return "Every day";
  if (d === "0,1,2,3,4") return "Weekdays";
  if (d === "5,6") return "Weekends";
  return r.days.map((i) => DAYS[i]).join(", ");
}

export function hhmm(h: number, m: number): string {
  return ("0" + h).slice(-2) + ":" + ("0" + m).slice(-2);
}

export function zoneName(zone: string): string {
  return zone.replace(/_/g, " ");
}

/** The offset from UTC of `zone` at `at`, in minutes (Panama is -300). 0 for an unknown zone. */
export function offsetMinutes(zone: string, at: Date): number {
  try {
    const name = new Intl.DateTimeFormat("en-US", { timeZone: zone, timeZoneName: "longOffset" })
      .formatToParts(at)
      .find((p) => p.type === "timeZoneName")?.value;
    const m = /GMT([+-])(\d{1,2})(?::(\d{2}))?/.exec(name ?? "");
    if (!m) return 0;
    const mins = Number(m[2]) * 60 + Number(m[3] ?? 0);
    return m[1] === "-" ? -mins : mins;
  } catch {
    return 0;
  }
}

/** "UTC-05:00", "UTC+05:30", "UTC". */
export function offsetText(minutes: number): string {
  if (minutes === 0) return "UTC";
  const a = Math.abs(minutes);
  return `UTC${minutes < 0 ? "-" : "+"}${String(Math.floor(a / 60)).padStart(2, "0")}:${String(a % 60).padStart(2, "0")}`;
}

export interface ZoneOption {
  id: string;
  /** For example "(UTC-05:00) America/Panama", with "changes with daylight saving" where the offset moves in the year. */
  label: string;
  offset: number;
}

/** Every zone with its offset now, ordered from west to east and then by name. A zone whose offset differs between
 *  winter and summer says so, because its offset is not the same all year. */
export function zoneOptions(ids: string[], now = new Date()): ZoneOption[] {
  const year = now.getUTCFullYear();
  const jan = new Date(Date.UTC(year, 0, 15));
  const jul = new Date(Date.UTC(year, 6, 15));
  return ids
    .map((id) => {
      const offset = offsetMinutes(id, now);
      const moves = offsetMinutes(id, jan) !== offsetMinutes(id, jul);
      return {
        id,
        offset,
        label: `(${offsetText(offset)}) ${zoneName(id)}${moves ? " (changes with daylight saving)" : ""}`,
      };
    })
    .sort((a, b) => a.offset - b.offset || a.id.localeCompare(b.id));
}
