// Small pieces several tabs share.
import type { ComponentChildren, JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import { say } from "./store.ts";

/**
 * One send at a time. `run(job)` starts the job unless one is already running (a double click, Enter held down),
 * and `busy` is true meanwhile so the button can be disabled.
 */
export function useBusy(): [boolean, (job: () => Promise<void>) => Promise<void>] {
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const run = async (job: () => Promise<void>) => {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    try {
      await job();
    } finally {
      running.current = false;
      setBusy(false);
    }
  };
  return [busy, run];
}

/** Copy `text` to the clipboard, with a fallback for pages served over plain http on a home network. */
export async function copyText(text: string): Promise<boolean> {
  if ("clipboard" in navigator && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }
  const ta = document.createElement("textarea");
  ta.setAttribute("aria-hidden", "true");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.append(ta);
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  ta.remove();
  return ok;
}

export function CopyButton(props: { text: string | (() => string); label?: string }): JSX.Element {
  const [done, setDone] = useState(false);
  const click = async (ev: Event) => {
    const button = ev.currentTarget as HTMLElement;
    const text = typeof props.text === "function" ? props.text() : props.text;
    const ok = await copyText(text);
    // The plain-http way of copying selects a hidden box, which takes the focus with it when it goes.
    if (document.activeElement !== button) button.focus({ preventScroll: true });
    if (ok) {
      setDone(true);
      setTimeout(() => setDone(false), 1500);
    } else say("Copy failed. Select the text and copy it by hand.", true);
  };
  return (
    <>
      <button class="plain" type="button" onClick={click}>
        {done ? "Copied" : (props.label ?? "Copy")}
      </button>
      <span class="sr" role="status">
        {done ? "Copied" : ""}
      </span>
    </>
  );
}

/** A row: the name and its hint on the left, the control on the right. The control must carry the same id. */
export function Field(props: { id: string; label: string; hint?: string; children: ComponentChildren }): JSX.Element {
  return (
    <div class="srow">
      <div class="slabel">
        <label for={props.id}>{props.label}</label>
        {props.hint ? <p class="mut">{props.hint}</p> : null}
      </div>
      <div class="sctl">{props.children}</div>
    </div>
  );
}

/** Offer `text` to the browser as a file to save (no request is made). */
export function saveFile(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
