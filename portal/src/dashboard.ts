// What the Dashboard says, as pure functions (unit-tested): the speaker's state in words, what needs attention with
// a fix link for each, why a test phrase cannot be spoken, and the merged list of recent happenings.
import { outcome, type Tone } from "./activity.ts";
import type { AgentEvent, AuditRecord, SpeakRefusal, SpeakerState, Status } from "./types.ts";

export type RingState = "idle" | "listening" | "thinking" | "speaking" | "muted" | "off" | "attention";

export interface SpeakerView {
  ring: RingState;
  /** One short sentence: the live state. */
  headline: string;
  /** Why, or what to do about it. */
  detail: string;
}

const WORDS: Record<SpeakerState, [string, RingState]> = {
  idle: ["Ready.", "idle"],
  listening: ["Listening.", "listening"],
  thinking: ["Thinking.", "thinking"],
  speaking: ["Speaking.", "speaking"],
  muted: ["Muted.", "muted"],
};

/** The speaker's state in plain words. `state` comes from the device when it reports one, otherwise it is worked out. */
export function speakerView(st: Status | null): SpeakerView {
  if (!st?.llm) return { ring: "off", headline: "Checking the speaker.", detail: "" };
  const sp = st.speaker;
  if (sp?.state && sp.state in WORDS) {
    const [headline, ring] = WORDS[sp.state];
    return { ring, headline, detail: "" };
  }
  if (st.muted === true) {
    return { ring: "muted", headline: "Muted.", detail: "It will not listen or speak until it is unmuted." };
  }
  if (sp?.connected === false) {
    return {
      ring: "off",
      headline: "No speaker is connected.",
      detail: "Nothing can be heard or said until one connects.",
    };
  }
  if (sp?.available === false) {
    return { ring: "idle", headline: "Busy or muted.", detail: "It is in a conversation, or muted." };
  }
  return { ring: "idle", headline: "Ready.", detail: "" };
}

export interface Attention {
  id: string;
  title: string;
  why: string;
  /** The place that fixes it (an address fragment), when the fix is on this page. */
  to?: string;
  label?: string;
}

/** What needs attention, most important first. An empty list means all is well. Unknown means not flagged. */
export function attention(st: Status | null, mode: string): Attention[] {
  if (!st?.llm) return [];
  const out: Attention[] = [];
  if (st.llm.configured === false) {
    out.push({
      id: "llm",
      title: "Language model not set",
      why: "Without one the speaker only understands simple commands such as timers.",
      to: "#/settings/assistant",
      label: "Set it up",
    });
  }
  if (st.speaker?.connected === false) {
    out.push({
      id: "speaker",
      title: "No speaker connected",
      why: "Start the speaker program on the device or on a computer so it can listen and talk.",
      to: "#/settings/about",
      label: "See the address",
    });
  }
  if (st.voice?.configured === false) {
    out.push({
      id: "voice",
      title: "No voice set up",
      why: "A speech provider is needed before the speaker can talk. Add one in the configuration file, then restart.",
      to: "#/settings/assistant",
      label: "See voice settings",
    });
  }
  if (st.clock_valid === false) {
    out.push({
      id: "clock",
      title: "Clock not set",
      why: "Reminders and alarms wait until the device gets the time from the network.",
      to: "#/settings/general",
      label: "Check the time zone",
    });
  }
  if (mode === "open") {
    out.push({
      id: "password",
      title: "No password",
      why: "Anyone on your network can open this page.",
      to: "#/settings/security",
      label: "Set a password",
    });
  }
  if (st.muted === true) {
    out.push({
      id: "muted",
      title: "Speaker muted",
      why: "It will not listen or speak. Press the speaker's mute button to turn it back on.",
    });
  }
  if (st.quiet_hours?.active === true) {
    out.push({
      id: "quiet",
      title: "Quiet hours are on",
      why: "Events from your inputs wait until the quiet window ends. Alarms and timers still ring.",
      to: "#/settings/safety",
      label: "Change quiet hours",
    });
  }
  return out;
}

export interface SpeakBlock {
  reason: SpeakRefusal;
  text: string;
  to?: string;
  label?: string;
}

const REFUSALS: Record<SpeakRefusal, Omit<SpeakBlock, "reason">> = {
  no_voice: {
    text: "No voice is set up, so the speaker has nothing to talk with.",
    to: "#/settings/assistant",
    label: "See voice settings",
  },
  no_speaker: {
    text: "No speaker is connected. Start the speaker program, then try again.",
    to: "#/settings/about",
    label: "See the address",
  },
  busy: { text: "The speaker is in a conversation or muted. Wait a moment, or unmute it." },
  muted: { text: "The speaker is muted. Press its mute button to turn it back on." },
  quiet_hours: {
    text: "Quiet hours are on.",
    to: "#/settings/safety",
    label: "Change quiet hours",
  },
};

export function isRefusal(reason: string): reason is SpeakRefusal {
  return reason in REFUSALS;
}

export function refusal(reason: SpeakRefusal): SpeakBlock {
  return { reason, ...REFUSALS[reason] };
}

/** Whether a test phrase is certain to be refused, from what the device last said (unknown means try it). */
export function speakBlock(st: Status | null): SpeakBlock | null {
  if (!st) return null;
  if (st.speaker?.name === "console") return null;
  if (st.voice?.configured === false) return refusal("no_voice");
  if (st.speaker?.connected === false) return refusal("no_speaker");
  if (st.muted === true) return refusal("muted");
  return null;
}

export interface Recent {
  key: string;
  ts: number;
  title: string;
  note: string;
  tone: Tone;
}

/** "set_timer" as people say it: "Set timer". */
export function toolName(tool: string): string {
  const t = tool.replace(/[_-]+/g, " ").trim();
  return t ? t.charAt(0).toUpperCase() + t.slice(1) : "Something";
}

/** The latest actions and events together, newest first. Text from events is data: it is only ever shown as text. */
export function recentItems(records: AuditRecord[], events: AgentEvent[], limit = 6): Recent[] {
  const items: Recent[] = [
    ...records.map((r): Recent => {
      const o = outcome(r);
      return { key: "a" + r.id, ts: r.ts, title: toolName(r.tool), note: o.label, tone: o.tone };
    }),
    ...events.map((e): Recent => ({
      key: "e" + e.id,
      ts: e.received_at,
      title: e.title,
      note: "from " + e.source,
      tone: "",
    })),
  ];
  return items.sort((a, b) => b.ts - a.ts).slice(0, limit);
}

/** The most recent time the speaker heard a person, from the audit log (voice turns), or null. */
export function lastHeard(records: AuditRecord[]): number | null {
  let best: number | null = null;
  for (const r of records) if (r.trigger === "voice" && (best === null || r.ts > best)) best = r.ts;
  return best;
}
