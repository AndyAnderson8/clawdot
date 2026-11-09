import type { JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { skillBody } from "../forms.ts";
import { CopyButton, Field } from "../parts.tsx";
import { change, loadExt, say, useApp, usePolled } from "../store.ts";
import type { Skill } from "../types.ts";

const MAX_IMPORT_BYTES = 70000;

export function Skills(): JSX.Element {
  const ext = useApp().ext;
  usePolled(loadExt);
  const skills = ext?.skills ?? [];
  const [editing, setEditing] = useState("");
  const [name, setName] = useState("");
  const [text, setText] = useState("");
  const [tools, setTools] = useState("");
  const [focusTick, setFocusTick] = useState(0);
  const [exported, setExported] = useState("");
  const [importText, setImportText] = useState("");
  const formRef = useRef<HTMLFormElement>(null);
  const exportRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!focusTick) return;
    formRef.current?.scrollIntoView({ block: "center" });
    document.getElementById("skill-name")?.focus();
  }, [focusTick]);
  useEffect(() => {
    if (exported) exportRef.current?.select();
  }, [exported]);

  const reset = () => {
    setEditing("");
    setName("");
    setText("");
    setTools("");
  };

  const submit = async (ev: Event) => {
    ev.preventDefault();
    const ok = editing
      ? await change(
          "POST",
          "/api/skills/edit",
          { name: editing, skill: skillBody(name, text, tools, skills.find((k) => k.name === editing)?.enabled) },
          "Saved.",
        )
      : await change(
          "POST",
          "/api/skills",
          skillBody(name, text, tools, true),
          "Added. The assistant uses it from now on.",
        );
    if (ok) reset();
  };

  const startEdit = (k: Skill) => {
    setEditing(k.name);
    setName(k.name);
    setText(k.instructions);
    setTools((k.tools ?? []).join(", "));
    setFocusTick((n) => n + 1);
  };

  const pick = (ev: Event) => {
    const input = ev.currentTarget as HTMLInputElement;
    const f = input.files?.[0];
    if (!f || f.size > MAX_IMPORT_BYTES) {
      say("Choose a skills file under 64 KB.", true);
      return;
    }
    const r = new FileReader();
    r.onload = () => setImportText(typeof r.result === "string" ? r.result : "");
    r.readAsText(f);
  };

  const doImport = async () => {
    let v: unknown;
    try {
      v = JSON.parse(importText);
    } catch {
      say("That is not a skills file. Paste the text that Export gave you.", true);
      return;
    }
    if (await change("POST", "/api/skills/import", v, "Imported. Read each skill, then turn on the ones you trust."))
      setImportText("");
  };

  return (
    <>
      <h2>Skills</h2>
      <p>
        A skill is a set of instructions you write for the assistant, for example “When I say good night, turn off the
        lights”. Turn skills on and off, or share them as files.
      </p>
      <div>
        {ext && !skills.length ? <p>No skills yet. Add one below, or import a file.</p> : null}
        {skills.length ? (
          <ul class="rows">
            {skills.map((k) => (
              <li key={k.name}>
                <div class="rowhead">
                  <strong>{k.name}</strong>
                  <span class={"badge" + (k.enabled ? " good" : "")}>{k.enabled ? "On" : "Off"}</span>
                </div>
                <div class="pre">{k.instructions}</div>
                {(k.tools ?? []).length ? <div class="mut">Tools: {(k.tools ?? []).join(", ")}</div> : null}
                <div class="acts">
                  <button
                    class="plain"
                    type="button"
                    onClick={() =>
                      change(
                        "POST",
                        "/api/skills/enabled",
                        { name: k.name, enabled: !k.enabled },
                        k.enabled ? "Turned off." : "Turned on.",
                      )
                    }
                  >
                    {k.enabled ? "Turn off" : "Turn on"}
                  </button>
                  <button class="plain" type="button" onClick={() => startEdit(k)}>
                    Edit
                  </button>
                  <button
                    class="danger"
                    type="button"
                    onClick={() => {
                      if (confirm("Delete the skill " + k.name + "?"))
                        void change("POST", "/api/skills/delete", { name: k.name }, "Deleted.");
                    }}
                  >
                    Delete
                  </button>
                </div>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      <form class="panel" ref={formRef} onSubmit={submit}>
        <h3>{editing ? "Change this skill" : "Add a skill"}</h3>
        <p class="mut">
          Write what the assistant should do, in your own words. It reads every skill that is turned on.
        </p>
        <Field id="skill-name" label="Name">
          <input
            id="skill-name"
            type="text"
            maxLength={40}
            placeholder="Good night"
            value={name}
            onInput={(e) => setName(e.currentTarget.value)}
          />
        </Field>
        <Field id="skill-text" label="Instructions">
          <textarea
            id="skill-text"
            maxLength={2000}
            rows={5}
            placeholder="When I say good night, turn off the lights and set an alarm for 7:00 on weekdays."
            value={text}
            onInput={(e) => setText(e.currentTarget.value)}
          />
        </Field>
        <p class="mut">{text.length} of 2000 characters</p>
        <Field
          id="skill-tools"
          label="Tools it uses (optional)"
          hint="Exact tool names, separated by commas. This only tells the assistant which tools to reach for; a tool that is off stays off."
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
        <div class="row">
          <button>{editing ? "Save changes" : "Add skill"}</button>
          {editing ? (
            <button class="plain" type="button" onClick={reset}>
              Stop editing
            </button>
          ) : null}
        </div>
      </form>
      <h3>Share skills</h3>
      <div class="row">
        <button
          class="plain"
          type="button"
          onClick={() => setExported(JSON.stringify({ format: "clawdot-skills", version: 1, skills }, null, 2))}
        >
          Export
        </button>
        <CopyButton text={() => exported} label="Copy export" />
      </div>
      <textarea
        ref={exportRef}
        readOnly
        rows={6}
        class={exported ? "" : "hidden"}
        aria-label="Exported skills"
        value={exported}
      />
      <p class="mut">
        Imported skills start turned off, because they are text from somewhere else. Read them before you turn them on.
      </p>
      <textarea
        rows={5}
        placeholder="Paste an exported skills file here"
        aria-label="Skills file to import"
        value={importText}
        onInput={(e) => setImportText(e.currentTarget.value)}
      />
      <div class="row">
        <input type="file" accept="application/json,.json" aria-label="Choose a skills file" onChange={pick} />
        <button type="button" onClick={doImport}>
          Import as turned off
        </button>
      </div>
    </>
  );
}
