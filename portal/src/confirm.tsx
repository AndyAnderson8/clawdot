// The one way to ask "are you sure?": a dialog in the page's own style, in place of the browser's confirm box. Any code
// can call `ask(...)` and await the answer; `ConfirmHost` is mounted once and shows the question.
import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { Dialog } from "./dialog.tsx";

interface Question {
  message: string;
  yes: string;
  no: string;
  done: (answer: boolean) => void;
}

let show: ((q: Question | null) => void) | null = null;

/**
 * Ask before doing something that cannot be undone. Resolves true only when the person presses the `yes` button;
 * Escape, Close and the `no` button are all a no.
 */
export function ask(message: string, yes = "OK", no = "Cancel"): Promise<boolean> {
  // Before the page is up there is nothing to show the question in, so the browser's own box is the fallback.
  if (!show) return Promise.resolve(window.confirm(message));
  const put = show;
  return new Promise((resolve) => put({ message, yes, no, done: resolve }));
}

/** Mounted once, near the top of the page. */
export function ConfirmHost(): JSX.Element {
  const [q, setQ] = useState<Question | null>(null);
  useEffect(() => {
    show = (next) =>
      setQ((old) => {
        // A second question while one is open answers the first with a no.
        old?.done(false);
        return next;
      });
    return () => {
      show = null;
    };
  }, []);
  const answer = (a: boolean) => {
    q?.done(a);
    setQ(null);
  };
  return (
    <Dialog title="Are you sure?" id="confirmdlg" open={q !== null} onClose={() => answer(false)}>
      {q ? (
        <>
          <p id="confirmtext">{q.message}</p>
          <div class="dlgfoot">
            <button class="plain" type="button" id="confirmno" onClick={() => answer(false)}>
              {q.no}
            </button>
            <button type="button" id="confirmyes" onClick={() => answer(true)}>
              {q.yes}
            </button>
          </div>
        </>
      ) : null}
    </Dialog>
  );
}
