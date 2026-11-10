import type { JSX } from "preact";
import { Field } from "../parts.tsx";
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
    if (!confirm("Forget this memory? The assistant will no longer know it.")) return;
    try {
      await api("DELETE", "/api/notes/" + id);
      say("Forgotten.");
      await load();
    } catch (x) {
      fail(x);
    }
  };
  const forgetAll = async () => {
    if (!confirm("Forget every memory? The assistant will no longer know any of them. This cannot be undone.")) return;
    try {
      await api("DELETE", "/api/notes");
      say("Everything forgotten.");
      await load();
    } catch (x) {
      fail(x);
    }
  };

  const max = data?.limits.max_note_chars ?? 200;
  const now = Math.floor(Date.now() / 1000);
  return (
    <>
      <p class="lead">
        Short facts you asked the assistant to remember, like “I'm allergic to peanuts” or “Mom's birthday is June 3”.
        It sees them every time it answers. You can also say “remember that ...” to the speaker. Never put a password or
        a key in a memory.
      </p>
      {missing ? (
        <div class="soon" role="status">
          <h3>Memory is not available</h3>
          <p>This device is not keeping a memory.</p>
        </div>
      ) : null}
      {data && !data.enabled ? (
        <div class="soon" role="status" id="notesoff">
          <h3>Memory is off</h3>
          <p>
            The assistant is not remembering anything. Turn it on under <a href="#/settings/features">Features</a>. What
            you already saved is kept.
          </p>
        </div>
      ) : null}
      {data ? (
        <>
          <form class="panel" id="noteform" onSubmit={add}>
            <h3>Add a memory</h3>
            <Field
              id="note-text"
              label="What to remember"
              hint={`${text.length} of ${max} characters. ${data.notes.length} of ${data.limits.max_notes} memories used.`}
            >
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
              <p class="errtext" role="alert">
                {err}
              </p>
            </Field>
            <div class="row">
              <button disabled={!data.enabled}>Save memory</button>
            </div>
          </form>
          <section class="card" id="notecard">
            <h3>
              What it remembers<span class="count">{data.notes.length}</span>
            </h3>
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
                        aria-label={"Forget this memory: " + n.text}
                        onClick={() => forget(n.id)}
                      >
                        Forget
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <p class="empty" id="noteempty">
                Nothing remembered yet. Add one above, or say “remember that ...” to the speaker.
              </p>
            )}
            {data.notes.length ? (
              <div class="row">
                <button class="danger" type="button" id="forgetall" onClick={forgetAll}>
                  Forget everything
                </button>
              </div>
            ) : null}
          </section>
        </>
      ) : null}
    </>
  );
}
