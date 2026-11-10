import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { fail, say, usePolled, zone } from "../store.ts";
import { whenText } from "../time.ts";
import type { NotesList } from "../types.ts";

export function Notes(): JSX.Element {
  const [data, setData] = useState<NotesList | null>(null);
  const [missing, setMissing] = useState(false);
  const [text, setText] = useState("");
  const [err, setErr] = useState("");

  const load = async () => {
    try {
      setData(await api<NotesList>("GET", "/api/notes"));
      setMissing(false);
    } catch (x) {
      if (x instanceof ApiError && x.status === 503) setMissing(true);
      else fail(x);
    }
  };
  usePolled(load);

  const add = async (ev: Event) => {
    ev.preventDefault();
    setErr("");
    try {
      setData(await api<NotesList>("POST", "/api/notes", { text }));
      setText("");
      say("Saved. The assistant will see it from now on.");
    } catch (x) {
      if (x instanceof ApiError && (x.status === 400 || x.status === 409)) setErr(x.message);
      else fail(x);
    }
  };
  const forget = async (id: number) => {
    if (!confirm("Forget this note? The assistant will no longer know it.")) return;
    try {
      await api("DELETE", "/api/notes/" + id);
      say("Forgotten.");
      await load();
    } catch (x) {
      fail(x);
    }
  };
  const forgetAll = async () => {
    if (!confirm("Forget every note? The assistant will no longer know any of them. This cannot be undone.")) return;
    try {
      await api("DELETE", "/api/notes");
      say("All notes forgotten.");
      await load();
    } catch (x) {
      fail(x);
    }
  };

  const max = data?.limits.max_note_chars ?? 200;
  const now = Math.floor(Date.now() / 1000);
  return (
    <>
      <p>
        Short facts you asked the assistant to remember, like “my printer is called Pixel”. It sees them every time it
        answers. You can also say “remember that ...” to the speaker. Never put a password or a key in a note.
      </p>
      {missing ? (
        <div class="soon" role="status">
          <h3>Notes are not available</h3>
          <p>This device is not keeping notes.</p>
        </div>
      ) : null}
      {data && !data.enabled ? (
        <div class="soon" role="status" id="notesoff">
          <h3>Notes are off</h3>
          <p>
            The assistant is not keeping or using notes. Turn them on under Keeping notes, further down this page. Notes
            you already saved are kept.
          </p>
        </div>
      ) : null}
      {data ? (
        <>
          <form class="panel" id="noteform" onSubmit={add}>
            <h3>Add a note</h3>
            <label for="note-text">What to remember</label>
            <input
              id="note-text"
              type="text"
              maxLength={max}
              autoComplete="off"
              placeholder="The spare key is under the blue pot"
              disabled={!data.enabled}
              value={text}
              onInput={(e) => setText(e.currentTarget.value)}
            />
            <p class="mut">
              {text.length} of {max} characters. {data.notes.length} of {data.limits.max_notes} notes used.
            </p>
            <p class="errtext" role="alert">
              {err}
            </p>
            <button disabled={!data.enabled}>Save note</button>
          </form>
          {data.notes.length ? (
            <ul class="rows" id="notelist">
              {data.notes.map((n) => (
                <li key={n.id}>
                  <div class="pre">{n.text}</div>
                  <div class="mut">Saved {whenText(n.ts, now, zone())}</div>
                  <div class="acts">
                    <button
                      class="danger"
                      type="button"
                      aria-label={"Forget this note: " + n.text}
                      onClick={() => forget(n.id)}
                    >
                      Forget
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <p id="noteempty">No notes yet. Add one above, or say “remember that ...” to the speaker.</p>
          )}
          {data.notes.length ? (
            <div class="row">
              <button class="danger" type="button" id="forgetall" onClick={forgetAll}>
                Forget all notes
              </button>
            </div>
          ) : null}
        </>
      ) : null}
    </>
  );
}
