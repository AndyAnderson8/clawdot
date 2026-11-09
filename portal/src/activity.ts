// The Activity view in plain words: what each audit record means, the filter query, and how arguments read.
// Pure functions, unit-tested. Everything in a record that came from outside is returned as text, never markup.
import type { AuditRecord } from "./types.ts";

export type Tone = "good" | "bad" | "warn" | "";

export interface Outcome {
  /** A short label for the badge. */
  label: string;
  /** One sentence that says what happened. */
  text: string;
  tone: Tone;
}

/** What happened to a call, as a label and a sentence. Unknown decisions (a newer core) are shown as written. */
export function outcome(r: Pick<AuditRecord, "decision" | "status">): Outcome {
  switch (r.decision) {
    case "ran":
      return r.status === "error"
        ? { label: "Failed", text: "It ran, but it reported an error.", tone: "bad" }
        : { label: "Ran", text: "It ran.", tone: "good" };
    case "held":
      return { label: "Held", text: "It is waiting for your yes. Nothing has happened yet.", tone: "warn" };
    case "proposed":
      return {
        label: "Suggested",
        text: "Something other than you asked for this, so it was kept as a suggestion until you say yes.",
        tone: "warn",
      };
    case "approved":
      return {
        label: "Approved",
        text: r.status === "error" ? "You said yes, but it reported an error." : "You said yes, so it ran.",
        tone: r.status === "error" ? "bad" : "good",
      };
    case "declined":
      return { label: "Declined", text: "You said no or asked for something else, so it was dropped.", tone: "" };
    case "expired":
      return { label: "Expired", text: "No answer came in time, so it was dropped.", tone: "" };
    case "cancelled":
      return { label: "Cancelled", text: "You cancelled it before it ran.", tone: "" };
    case "blocked:cap":
      return { label: "Blocked", text: "Not run: the daily limit on actions was already used up.", tone: "bad" };
    case "blocked:dry-run":
      return { label: "Dry run", text: "Not run: dry run is on, so it was only described.", tone: "warn" };
    case "blocked:invalid":
      return {
        label: "Blocked",
        text: "Not run: the request was not valid or named a tool that is not offered.",
        tone: "bad",
      };
    case "blocked:pending":
      return { label: "Blocked", text: "Not run: another action was waiting for your yes.", tone: "bad" };
    case "blocked:limit":
      return {
        label: "Stopped",
        text: "Not run: the request had reached its step, time or failure limit.",
        tone: "bad",
      };
    default:
      return { label: r.decision || "Unknown", text: "This record has a result this page does not know.", tone: "" };
  }
}

const TIERS: Record<string, string> = {
  read_only: "Only reads",
  reversible: "Can be undone",
  costly: "Asks you first",
  destructive: "Destructive",
};
export function tierLabel(tier: string): string {
  return TIERS[tier] ?? tier;
}

/** Who started the turn. */
export function triggerLabel(trigger: string): string {
  if (trigger === "voice") return "You asked by voice";
  if (trigger.startsWith("event:")) return "Event from " + trigger.slice(6);
  if (trigger === "event") return "An event";
  return trigger;
}

export interface AuditFilters {
  tool: string;
  tier: string;
  decision: string;
  trigger: string;
}
export const NO_FILTERS: AuditFilters = { tool: "", tier: "", decision: "", trigger: "" };

/** The query string for `GET /api/audit`: only the filters that are set, plus the page position. */
export function auditQuery(f: AuditFilters, before: number | null, limit = 25): string {
  const p = new URLSearchParams();
  p.set("limit", String(limit));
  if (before !== null) p.set("before", String(before));
  for (const k of ["tool", "tier", "decision", "trigger"] as const) {
    const v = f[k].trim();
    if (v) p.set(k, v);
  }
  return p.toString();
}

/** The arguments as `name: value` lines. Objects and lists inside a value are shown as compact JSON. */
export function argsText(args: unknown): string {
  if (args === null || args === undefined) return "";
  if (typeof args !== "object" || Array.isArray(args)) return JSON.stringify(args);
  const lines = Object.entries(args as Record<string, unknown>).map(([k, v]) => {
    const text = typeof v === "string" ? v : JSON.stringify(v);
    return k + ": " + text;
  });
  return lines.join("\n");
}

/** `12.3 KB` style size. */
export function kb(bytes: number): string {
  return bytes < 1024 ? bytes + " B" : (bytes / 1024).toFixed(1) + " KB";
}

/** How a tier reads in the list a person chooses from. */
export function tierChoice(tier: string): string {
  switch (tier) {
    case "read_only":
      return "Only reads. Runs right away";
    case "reversible":
      return "Can be undone. Runs right away";
    case "costly":
      return "Asks you out loud first";
    default:
      return tierLabel(tier);
  }
}

/** The question to answer before a tool is allowed to run without asking, or "" when nothing is relaxed. */
export function relaxWarning(name: string, from: string, to: string): string {
  const rank = ["read_only", "reversible", "costly", "destructive"];
  if (rank.indexOf(to) >= rank.indexOf(from)) return "";
  return (
    "Let " +
    name +
    " run right away without asking you first? If something it reads, such as a web page or a message, tries to trick the assistant, ClawDot still asks you first in that conversation, but otherwise this tool will act on a spoken request alone."
  );
}
