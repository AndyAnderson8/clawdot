import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api } from "../api.ts";
import { AddButton, Dialog } from "../dialog.tsx";
import {
  SKILL_NAME_MAX,
  SKILL_TEXT_MAX,
  matching,
  readSkillFiles,
  skillBody,
  type SkillDraft,
  type SkillFile,
} from "../forms.ts";
import { Field } from "../parts.tsx";
import { change, fail, loadExt, say, useApp, usePolled } from "../store.ts";
import type { Skill } from "../types.ts";

const MAX_FILE_BYTES = 70000;
const MAX_FILES = 300;

/** Make a skill, or change one in place. `skill` is the one being changed; none means a new one. */
function SkillForm(props: { skill: Skill | null; onDone: () => void }): JSX.Element {
  const k = props.skill;
  const [name, setName] = useState(k?.name ?? "");
  const [text, setText] = useState(k?.instructions ?? "");
  const [tools, setTools] = useState((k?.tools ?? []).join(", "));
  const submit = async (ev: Event) => {
    ev.preventDefault();
    const ok = k
      ? await change(
          "POST",
          "/api/skills/edit",
          { name: k.name, skill: skillBody(name, text, tools, k.enabled) },
          "Saved.",
        )
      : await change(
          "POST",
          "/api/skills",
          skillBody(name, text, tools, true),
          "Added. The assistant uses it from now on.",
        );
    if (ok) props.onDone();
  };
  const remove = () => {
    if (k && confirm("Delete the skill " + k.name + "?"))
      void change("POST", "/api/skills/delete", { name: k.name }, "Deleted.").then((ok) => ok && props.onDone());
  };
  return (
    <form class="dlgform" onSubmit={submit}>
      <p class="mut">Write what the assistant should do, in your own words. It reads every skill that is turned on.</p>
      <Field id="skill-name" label="Name">
        <input
          id="skill-name"
          type="text"
          maxLength={SKILL_NAME_MAX}
          placeholder="Good night"
          value={name}
          onInput={(e) => setName(e.currentTarget.value)}
        />
      </Field>
      <Field id="skill-text" label="Instructions">
        <textarea
          id="skill-text"
          maxLength={SKILL_TEXT_MAX}
          rows={8}
          placeholder="When I say good night, turn off the lights and set an alarm for 7:00 on weekdays."
          value={text}
          onInput={(e) => setText(e.currentTarget.value)}
        />
        <p class="mut">
          {text.length} of {SKILL_TEXT_MAX} characters
        </p>
      </Field>
      <Field
        id="skill-tools"
        label="Tools it uses"
        hint="Optional. Exact tool names, separated by commas. This only tells the assistant which tools to reach for; a tool that is off stays off."
      >
        <input
          id="skill-tools"
          type="text"
          placeholder="set_light, set_alarm"
          autoCapitalize="none"
          autoComplete="off"
          value={tools}
          onInput={(e) => setTools(e.currentTarget.value)}
        />
      </Field>
      <div class="row dlgfoot">
        <button>{k ? "Save changes" : "Add skill"}</button>
        <button class="plain" type="button" onClick={props.onDone}>
          Cancel
        </button>
        {k ? (
          <button class="danger" type="button" id="skill-delete" onClick={remove}>
            Delete
          </button>
        ) : null}
      </div>
    </form>
  );
}

/** Skills from files on this computer: a folder, or one or more files. The browser reads them; nothing is uploaded until you add. */
function SkillImport(props: { onDone: () => void }): JSX.Element {
  const [list, setList] = useState<{ found: SkillDraft[]; skipped: string[] } | null>(null);
  const [off, setOff] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const read = async (files: FileList | null) => {
    if (!files?.length) return;
    const all = Array.from(files).filter((f) => /\.(md|json)$/i.test(f.name));
    if (all.length > MAX_FILES) {
      say("That folder has more than " + MAX_FILES + " files to read. Choose a smaller folder.", true);
      return;
    }
    const skippedBig: string[] = [];
    const read: SkillFile[] = [];
    for (const f of all) {
      const path = f.webkitRelativePath || f.name;
      if (f.size > MAX_FILE_BYTES) skippedBig.push(path + ": it is larger than 64 KB");
      else read.push({ path, text: await f.text() });
    }
    const r = readSkillFiles(read);
    setList({ found: r.found, skipped: [...skippedBig, ...r.skipped] });
    setOff([]);
  };
  const add = async () => {
    if (!list) return;
    setBusy(true);
    const skills = list.found.filter((s) => !off.includes(s.name));
    try {
      const out = await api<unknown>("POST", "/api/skills/import", { format: "clawdot-skills", version: 1, skills });
      void out;
      await loadExt();
      say(
        "Added " +
          skills.length +
          (skills.length === 1 ? " skill" : " skills") +
          ", turned off. Read each one, then turn on the ones you trust.",
      );
      props.onDone();
    } catch (x) {
      fail(x);
    }
    setBusy(false);
  };
  const chosen = list ? list.found.length - off.length : 0;
  return (
    <div class="dlgform">
      <p class="mut">
        Point at a folder of skills (each in its own folder with a SKILL.md file), or at single files. ClawDot reads
        them here in your browser and lists what it found. Added skills start turned off, because they are text from
        somewhere else.
      </p>
      <div class="row">
        <label class="plain small filebtn">
          Choose a folder
          <input
            id="skillfolder"
            type="file"
            class="sr"
            ref={(el) => el?.setAttribute("webkitdirectory", "")}
            onChange={(e) => void read(e.currentTarget.files)}
          />
        </label>
        <label class="plain small filebtn">
          Choose files
          <input
            id="skillfiles"
            type="file"
            class="sr"
            multiple
            accept=".md,.json,text/markdown,application/json"
            onChange={(e) => void read(e.currentTarget.files)}
          />
        </label>
      </div>
      {list ? (
        <fieldset class="pickbox" id="skillfound">
          <legend>
            Found {list.found.length} {list.found.length === 1 ? "skill" : "skills"}
          </legend>
          {list.found.length ? (
            <>
              <div class="row">
                <button class="plain small" type="button" onClick={() => setOff([])}>
                  Select all
                </button>
                <button class="plain small" type="button" onClick={() => setOff(list.found.map((s) => s.name))}>
                  Select none
                </button>
                <span class="mut" role="status">
                  {chosen} of {list.found.length} selected
                </span>
              </div>
              <ul class="picklist">
                {list.found.map((s) => (
                  <li key={s.name}>
                    <label class="inline">
                      <input
                        type="checkbox"
                        checked={!off.includes(s.name)}
                        onChange={(e) =>
                          setOff(e.currentTarget.checked ? off.filter((x) => x !== s.name) : [...off, s.name])
                        }
                      />{" "}
                      <span>
                        <strong>{s.name}</strong>
                        <span class="mut skillline">{s.instructions.slice(0, 120)}</span>
                      </span>
                    </label>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p class="mut">No skills found there.</p>
          )}
          {list.skipped.length ? (
            <details class="help">
              <summary>{list.skipped.length} left out</summary>
              <ul>
                {list.skipped.map((s) => (
                  <li key={s}>{s}</li>
                ))}
              </ul>
            </details>
          ) : null}
        </fieldset>
      ) : null}
      <div class="row dlgfoot">
        <button type="button" id="skilladd" disabled={!chosen || busy} onClick={() => void add()}>
          {chosen ? "Add " + chosen + (chosen === 1 ? " skill" : " skills") : "Add skills"}
        </button>
        <button class="plain" type="button" onClick={props.onDone}>
          Cancel
        </button>
      </div>
    </div>
  );
}

export function Skills(): JSX.Element {
  const ext = useApp().ext;
  usePolled(loadExt);
  const skills = ext?.skills ?? [];
  const [query, setQuery] = useState("");
  const [adding, setAdding] = useState<"" | "write" | "import">("");
  const [editing, setEditing] = useState<Skill | null>(null);
  const shown = matching(skills, query, (k) => k.instructions);
  const on = skills.filter((k) => k.enabled).length;

  // Turn a group of skills on or off at once.
  const setMany = async (list: Skill[], enabled: boolean) => {
    const todo = list.filter((k) => k.enabled !== enabled);
    try {
      for (const k of todo) await api("POST", "/api/skills/enabled", { name: k.name, enabled });
    } catch (x) {
      fail(x);
    }
    await loadExt();
    if (todo.length)
      say(todo.length + (todo.length === 1 ? " skill turned " : " skills turned ") + (enabled ? "on." : "off."));
  };
  const toggle = (k: Skill) =>
    change(
      "POST",
      "/api/skills/enabled",
      { name: k.name, enabled: !k.enabled },
      k.enabled ? "Turned off." : "Turned on.",
    );

  return (
    <>
      <section class="card" id="skillcard">
        <div class="head">
          <h3>
            Skills<span class="count">{skills.length}</span>
          </h3>
          <AddButton label="Add skills" id="addskill" onClick={() => setAdding("write")} />
        </div>
        <p class="mut">
          Instructions that teach the assistant a routine, for example “When I say good night, turn off the lights”. It
          reads every skill that is turned on.
        </p>
        {skills.length ? (
          <div class="toolbar">
            <input
              id="skillsearch"
              type="search"
              placeholder="Search skills"
              aria-label="Search skills"
              value={query}
              onInput={(e) => setQuery(e.currentTarget.value)}
            />
            <button class="plain small" type="button" id="skillall" onClick={() => void setMany(shown, true)}>
              Turn on {query ? "these" : "all"}
            </button>
            <button class="plain small" type="button" id="skillnone" onClick={() => void setMany(shown, false)}>
              Turn off {query ? "these" : "all"}
            </button>
            <span class="mut" role="status" id="skillcount">
              {on} of {skills.length} turned on
            </span>
          </div>
        ) : null}
        {ext && !skills.length ? (
          <p class="empty">No skills yet. Use “Add skills” to write one or to bring them in from a folder.</p>
        ) : ext && !shown.length ? (
          <p class="empty">No skill matches that.</p>
        ) : (
          <ul class="rows tools">
            {shown.map((k) => (
              <li key={k.name} class={"toolrow skillrow" + (k.enabled ? "" : " off")}>
                <input
                  class="switch"
                  type="checkbox"
                  checked={k.enabled}
                  aria-label={k.name + (k.enabled ? " is on" : " is off")}
                  onChange={() => void toggle(k)}
                />
                <div class="toolmain">
                  <div class="skillname">{k.name}</div>
                  <div class="tooldesc clamp">{k.instructions}</div>
                  {(k.tools ?? []).length ? <div class="toolorigin">Uses {(k.tools ?? []).join(", ")}</div> : null}
                </div>
                <div class="toolact">
                  <button class="plain small" type="button" onClick={() => setEditing(k)}>
                    Edit
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      <Dialog id="skilladddialog" title="Add skills" open={adding !== ""} onClose={() => setAdding("")} wide>
        <div class="seg" role="group" aria-label="How to add">
          <button
            type="button"
            class="plain small"
            aria-pressed={adding === "write"}
            onClick={() => setAdding("write")}
          >
            Write one
          </button>
          <button
            type="button"
            class="plain small"
            aria-pressed={adding === "import"}
            onClick={() => setAdding("import")}
          >
            From a folder or files
          </button>
        </div>
        {adding === "write" ? <SkillForm skill={null} onDone={() => setAdding("")} /> : null}
        {adding === "import" ? <SkillImport onDone={() => setAdding("")} /> : null}
      </Dialog>
      <Dialog
        id="skilleditdialog"
        title={editing ? "Edit " + editing.name : "Edit skill"}
        open={editing !== null}
        onClose={() => setEditing(null)}
        wide
      >
        {editing ? <SkillForm key={editing.name} skill={editing} onDone={() => setEditing(null)} /> : null}
      </Dialog>
    </>
  );
}
