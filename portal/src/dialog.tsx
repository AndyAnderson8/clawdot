// The one way to add something, everywhere: a "+ Add" button in the card's header opens a dialog with its form. The dialog
// is the browser's own <dialog> element, so it traps focus, closes with Escape and is announced as a dialog. It also has a Close button.
import type { ComponentChildren, JSX } from "preact";
import { useEffect, useRef } from "preact/hooks";

/** A small "+ Add ..." button for a card's header. */
export function AddButton(props: { label: string; onClick: () => void; id?: string }): JSX.Element {
  return (
    <button class="plain small addbtn" type="button" id={props.id} onClick={props.onClick}>
      <span aria-hidden="true">+</span> {props.label}
    </button>
  );
}

/** A modal box. It is shown while `open` is true; closing it (Escape or the Close button) calls `onClose`. */
export function Dialog(props: {
  title: string;
  open: boolean;
  onClose: () => void;
  children: ComponentChildren;
  /** A wider box for lists. */
  wide?: boolean;
  id?: string;
}): JSX.Element {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (props.open && !el.open) {
      if (typeof el.showModal === "function") el.showModal();
      else el.setAttribute("open", "");
    } else if (!props.open && el.open) el.close();
  }, [props.open]);
  return (
    <dialog
      ref={ref}
      id={props.id}
      class={"dlg" + (props.wide ? " wide" : "")}
      aria-labelledby={(props.id ?? "dlg") + "-title"}
      onClose={props.onClose}
    >
      {props.open ? (
        <div class="dlgbox">
          <header class="dlghead">
            <h3 id={(props.id ?? "dlg") + "-title"}>{props.title}</h3>
            <button class="plain small" type="button" aria-label="Close" onClick={props.onClose}>
              Close
            </button>
          </header>
          {props.children}
        </div>
      ) : null}
    </dialog>
  );
}
