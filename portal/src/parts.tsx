// Small pieces several tabs share.
import type { ComponentChildren } from "preact";
import { useState } from "preact/hooks";
import { say } from "./store.ts";

/** Copy `text` to the clipboard, with a fallback for pages served over plain http on a home network. */
export async function copyText(text: string): Promise<boolean> {
  if (navigator.clipboard && window.isSecureContext) {
    try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
  }
  const ta = document.createElement("textarea");
  ta.setAttribute("aria-hidden", "true");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.append(ta);
  ta.select();
  let ok = false;
  try { ok = document.execCommand("copy"); } catch { ok = false; }
  ta.remove();
  return ok;
}

export function CopyButton(props: { text: string | (() => string); label?: string }) {
  const [done, setDone] = useState(false);
  const click = async () => {
    const text = typeof props.text === "function" ? props.text() : props.text;
    if (await copyText(text)) {
      setDone(true);
      setTimeout(() => setDone(false), 1500);
    } else say("Copy failed. Select the text and copy it by hand.", true);
  };
  return <button class="plain" type="button" onClick={click}>{done ? "Copied" : props.label ?? "Copy"}</button>;
}

/** A label, a control, and an optional hint line. The control must carry the same id. */
export function Field(props: { id: string; label: string; hint?: string; children: ComponentChildren }) {
  return (
    <div>
      <label for={props.id}>{props.label}</label>
      {props.children}
      {props.hint ? <p class="mut">{props.hint}</p> : null}
    </div>
  );
}

export function ConfirmBadge(props: { confirm: boolean }) {
  return <span class={"badge" + (props.confirm ? " good" : "")}>{props.confirm ? "Asks you first" : "Runs right away"}</span>;
}

export function OffBadge(props: { enabled: boolean }) {
  return props.enabled ? null : <span class="badge bad">Off</span>;
}

export function go(tab: string): void { location.hash = "#/" + tab; }
