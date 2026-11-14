import type { JSX } from "preact";
import { Field, useBusy } from "../parts.tsx";
import { AddButton, Dialog } from "../dialog.tsx";
import { useEffect, useState } from "preact/hooks";
import { api, ApiError, enc } from "../api.ts";
import { attempt, deviceNow, fail, say, usePolled, useSeq, zone } from "../store.ts";
import { whenText } from "../time.ts";
import type { Note, NotesList } from "../types.ts";
import { ask } from "../confirm.tsx";

/** What the assistant remembers: short facts, listed like tools, each with a pencil to change it and an X to forget it. */
export function Notes(): JSX.Element {
  const [data, setData] = useState<NotesList | null>(null);
  const [missing, setMissing] = useState(false);
  const [text, setText] = useState("");
  const [err, setErr] = useState("");
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<Note | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Note[] | null>(null);
  const [busy, run] = useBusy();

  // Searching asks the device, which ranks memories the same way it picks them for a request. A short wait after
  // typing keeps it from asking on every key.
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits(null);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      api<NotesList>("GET", "/api/notes?q=" + enc(q))
        .then((r) => {
          if (live) setHits(r.notes);
        })
        .catch(fail);
    }, 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [query, data]);

  const next = useSeq();
  const load = async () => {
    const current = next();
    try {
      const list = await api<NotesList>("GET", "/api/notes");
      if (!current()) return;
      setData(list);
      setMissing(false);
    } catch (x) {
      if (x instanceof ApiError && x.status === 503) setMissing(true);
      else fail(x);
    }
  };
  usePolled(load);

  const close = () => {
    setOpen(false);
    setEditing(null);
    setText("");
    setErr("");
  };
  const save = async (ev: Event) => {
    ev.preventDefault();
    setErr("");
    await run(async () => {
      try {
        const list = editing
          ? await api<NotesList>("PUT", "/api/notes/" + enc(editing.id), { text })
          : await api<NotesList>("POST", "/api/notes", { text });
        next();
        setData(list);
        say(editing ? "Saved." : "Saved. The assistant will see it from now on.");
        close();
      } catch (x) {
        if (x instanceof ApiError && [400, 404, 409].includes(x.status)) setErr(x.message);
        else fail(x);
      }
    });
  };
  const forget = async (id: number) => {
    if (!(await ask("Forget this memory? The assistant will no longer know it.", "Forget"))) return;
    await attempt(async () => {
      await api("DELETE", "/api/notes/" + enc(id));
      say("Forgotten.");
      await load();
    });
  };
  const forgetAll = async () => {
    const sure = await ask(
      "Forget every memory? The assistant will no longer know any of them. This cannot be undone.",
      "Forget all",
    );
    if (!sure) return;
    await attempt(async () => {
      await api("DELETE", "/api/notes");
      say("Everything forgotten.");
      await load();
    });
  };

  const max = data?.limits.max_note_chars ?? 200;
  const now = deviceNow();
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
          <section class="card" id="notecard">
            <div class="head">
              <h3>
                What it remembers<span class="count">{data.notes.length}</span>
              </h3>
              <span class="headacts" title={data.enabled ? undefined : "Turn memory on under Features first."}>
                {data.notes.length ? (
                  <button class="plain small" type="button" id="forgetall" onClick={forgetAll}>
                    Forget everything
                  </button>
                ) : null}
                <AddButton
                  label="Add a memory"
                  id="addnote"
                  onClick={() => {
                    setEditing(null);
                    setText("");
                    setOpen(true);
                  }}
                  disabled={!data.enabled}
                />
              </span>
            </div>
            {data.notes.length > 5 || query ? (
              <div class="toolbar">
                <input
                  id="notesearch"
                  type="search"
                  placeholder="Search memories"
                  aria-label="Search memories"
                  value={query}
                  onInput={(e) => setQuery(e.currentTarget.value)}
                />
                {hits ? (
                  <span class="mut" role="status" id="notehits">
                    {hits.length} of {data.notes.length} match
                  </span>
                ) : null}
              </div>
            ) : null}
            {hits && !hits.length ? (
              <p class="empty" id="notenone">
                No memory matches that.
              </p>
            ) : data.notes.length ? (
              <ul class="rows tools" id="notelist">
                {(hits ?? data.notes).map((n) => (
                  <li key={n.id} class="toolrow memrow">
                    <div class="toolmain">
                      <div class="memtext">{n.text}</div>
                      <div class="toolorigin">Saved {whenText(n.ts, now, zone())}</div>
                    </div>
                    <div class="iconacts">
                      <button
                        class="iconbtn"
                        type="button"
                        title="Change this memory"
                        aria-label={"Change this memory: " + n.text}
                        disabled={!data.enabled}
                        onClick={() => {
                          setEditing(n);
                          setText(n.text);
                          setOpen(true);
                        }}
                      >
                        <svg viewBox="0 0 24 24" aria-hidden="true">
                          <path d="m4 20 1-4L16.500 4.500a2.100 2.100 0 0 1 3 3L8 19ZM14 7l3 3" />
                        </svg>
                      </button>
                      <button
                        class="iconbtn danger"
                        type="button"
                        title="Forget this memory"
                        aria-label={"Forget this memory: " + n.text}
                        onClick={() => forget(n.id)}
                      >
                        <svg viewBox="0 0 24 24" aria-hidden="true">
                          <path d="M6 6l12 12M18 6 6 18" />
                        </svg>
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <p class="empty" id="noteempty">
                Nothing remembered yet. Use “Add a memory”, or say “remember that ...” to the speaker.
              </p>
            )}
          </section>
          <Dialog id="notedialog" title={editing ? "Change this memory" : "Add a memory"} open={open} onClose={close}>
            <form class="dlgform" id="noteform" onSubmit={save}>
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
              <div class="row dlgfoot">
                <button disabled={!data.enabled || busy}>{editing ? "Save changes" : "Save memory"}</button>
                <button class="plain" type="button" onClick={close}>
                  Cancel
                </button>
              </div>
            </form>
          </Dialog>
        </>
      ) : null}
    </>
  );
}
