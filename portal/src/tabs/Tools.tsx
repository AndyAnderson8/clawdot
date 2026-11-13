import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { relaxWarning, tierLabel } from "../activity.ts";
import { AddButton, Dialog } from "../dialog.tsx";
import { Field } from "../parts.tsx";
import { change, loadExt, say, useApp, usePolled } from "../store.ts";
import { matching, toolDefinition, type ParamForm } from "../forms.ts";
import type { Extensions, Tool } from "../types.ts";
import { ask } from "../confirm.tsx";

/** The "Saved, but not running" box for tools or sources that failed to load. */
export function Problems(props: { ext: Extensions; kind: string }): JSX.Element | null {
  const list = (props.ext.problems ?? []).filter((x) => x.kind === props.kind);
  if (!list.length) return null;
  return (
    <div class="soon">
      <h3>Saved, but not running</h3>
      {list.map((x) => (
        <p key={x.name}>
          {x.name}: {x.reason}. Fix that and restart ClawDot.
        </p>
      ))}
    </div>
  );
}

/** The short words for how a tool is run. */
function whenWords(tier: string): string {
  switch (tier) {
    case "read_only":
      return "Runs, only reads";
    case "reversible":
      return "Runs, can be undone";
    case "costly":
      return "Asks you first";
    default:
      return tierLabel(tier);
  }
}

/** Does the assistant ask out loud before using the tool, or just use it? Only some tools let you change that. */
function WhenItRuns(props: { t: Tool }): JSX.Element {
  const { t } = props;
  const id = "tier-" + t.name;
  // The box shows the pick at once, so a pick that is declined or does not save is put back to the real level.
  const pick = async (sel: HTMLSelectElement) => {
    const to = sel.value;
    const warn = relaxWarning(t.name, t.tier ?? "costly", to);
    const saved =
      !(warn && !(await ask(warn, "Change it"))) &&
      (await change(
        "POST",
        "/api/tools/" + t.name + "/tier",
        { tier: to === "" ? null : to },
        to === "" ? "Back to the usual level." : "Saved. It applies from the next request.",
      ));
    if (!saved) sel.value = t.tier ?? "";
  };
  if (!(t.tier && t.tier_options?.length)) {
    return (
      <span class={"when" + (t.confirm ? " ask" : "")}>
        {t.tier ? whenWords(t.tier) : t.confirm ? "Asks you first" : "Runs right away"}
      </span>
    );
  }
  return (
    <div>
      <label class="sr" for={id}>
        How {t.name} runs
      </label>
      <select id={id} class="whensel" value={t.tier} onChange={(e) => void pick(e.currentTarget)}>
        {(t.tier_options ?? []).map((o) => (
          <option key={o} value={o}>
            {whenWords(o)}
          </option>
        ))}
        {t.tier_set ? (
          <option value="">Back to the usual level ({whenWords(t.tier_default ?? "costly")})</option>
        ) : null}
        <option value="destructive" disabled>
          Destructive: not available here
        </option>
      </select>
      {t.tier_set ? (
        <p class="mut tiernote">Changed by you. Usually “{whenWords(t.tier_default ?? "costly")}”.</p>
      ) : null}
    </div>
  );
}

/** Where a tool comes from, in a few words. */
function origin(t: Tool): string {
  if (t.source === "builtin") return "Built in";
  if (t.source === "mcp") return "From " + (t.server ?? "a source");
  return "Added by you";
}

function ToolRow(props: { t: Tool }) {
  const t = props.t;
  const remove = async () => {
    if (await ask("Delete the tool " + t.name + "? The assistant will stop being able to use it.", "Delete"))
      void change("DELETE", "/api/tools/" + t.name, undefined, "Deleted.");
  };
  return (
    <li class={"toolrow" + (t.enabled ? "" : " off")}>
      <input
        id={"on-" + t.name}
        class="switch"
        type="checkbox"
        checked={t.enabled}
        aria-label={t.name + (t.enabled ? " is on" : " is off")}
        onChange={() =>
          change(
            "POST",
            "/api/tools/" + t.name + "/enabled",
            { enabled: !t.enabled },
            t.name + (t.enabled ? " turned off." : " turned on."),
          )
        }
      />
      <div class="toolmain">
        <div class="toolname">{t.name}</div>
        <div class="tooldesc">{t.description}</div>
        <div class="toolorigin">{origin(t)}</div>
      </div>
      <div class="toolwhen">
        <WhenItRuns t={t} />
      </div>
      <div class="toolact">
        {t.deletable ? (
          <button class="danger small" type="button" onClick={remove}>
            Delete
          </button>
        ) : null}
      </div>
    </li>
  );
}

/** Tools as rows. */
export function ToolList(props: { tools: Tool[] }): JSX.Element {
  return (
    <ul class="rows tools">
      {props.tools.map((t) => (
        <ToolRow key={t.name} t={t} />
      ))}
    </ul>
  );
}

const TYPES: [string, string][] = [
  ["string", "Text"],
  ["integer", "Whole number"],
  ["number", "Number"],
  ["boolean", "Yes or no"],
];
const METHODS = ["GET", "POST", "PUT", "DELETE"];
let nextParam = 1;

/** Make a tool from scratch: one fixed web address the assistant may call. */
function AddTool(props: { open: boolean; onClose: () => void }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [method, setMethod] = useState("POST");
  const [url, setUrl] = useState("");
  const [tokenVar, setTokenVar] = useState("");
  const [confirmFirst, setConfirmFirst] = useState(true);
  const [params, setParams] = useState<ParamForm[]>([]);
  const edit = (id: number, patch: Partial<ParamForm>) =>
    setParams(params.map((p) => (p.id === id ? { ...p, ...patch } : p)));

  const submit = async (ev: Event) => {
    ev.preventDefault();
    const def = toolDefinition({ name, description, method, url, tokenVar, confirm: confirmFirst, params });
    if (typeof def === "string") {
      say(def, true);
      return;
    }
    if (await change("POST", "/api/tools", def, "Added. The assistant can use it now.")) {
      setName("");
      setDescription("");
      setMethod("POST");
      setUrl("");
      setTokenVar("");
      setConfirmFirst(true);
      setParams([]);
      props.onClose();
    }
  };

  return (
    <Dialog id="tooldialog" title="Add a tool" open={props.open} onClose={props.onClose} wide>
      <form class="dlgform" onSubmit={submit}>
        <p class="mut">
          Make a tool from a web address. When the assistant uses it, ClawDot calls that one address. The assistant can
          choose the details you list, but cannot change the address or the token.
        </p>
        <Field id="tool-name" label="Name" hint="Lowercase letters, digits and underscores.">
          <input
            id="tool-name"
            type="text"
            maxLength={40}
            autoCapitalize="none"
            autoComplete="off"
            placeholder="set_light"
            value={name}
            onInput={(e) => setName(e.currentTarget.value)}
          />
        </Field>
        <Field id="tool-desc" label="What it does" hint="The assistant reads this to decide when to use it.">
          <input
            id="tool-desc"
            type="text"
            maxLength={300}
            placeholder="Turn a room's light on or off"
            value={description}
            onInput={(e) => setDescription(e.currentTarget.value)}
          />
        </Field>
        <Field
          id="tool-url"
          label="Web address"
          hint="Where a detail goes, write its name in braces, for example {room}."
        >
          <input
            id="tool-url"
            type="text"
            inputMode="url"
            autoCapitalize="none"
            autoComplete="off"
            placeholder="https://home.local/api/light/{room}"
            value={url}
            onInput={(e) => setUrl(e.currentTarget.value)}
          />
        </Field>
        <Field id="tool-method" label="Request type" hint="Most services use POST to make a change and GET to read.">
          <select id="tool-method" value={method} onChange={(e) => setMethod(e.currentTarget.value)}>
            {METHODS.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </Field>
        <Field
          id="tool-token"
          label="Token variable"
          hint="Optional. The name of an environment variable on the device that holds the service's token. Type the name only, never the token itself."
        >
          <input
            id="tool-token"
            type="text"
            maxLength={64}
            autoCapitalize="none"
            autoComplete="off"
            placeholder="HA_TOKEN"
            value={tokenVar}
            onInput={(e) => setTokenVar(e.currentTarget.value)}
          />
        </Field>
        <div class="row">
          <input
            type="checkbox"
            id="tool-asks"
            checked={confirmFirst}
            onChange={(e) => setConfirmFirst(e.currentTarget.checked)}
          />
          <label for="tool-asks" class="inline">
            Ask me out loud before it runs (recommended)
          </label>
        </div>
        <h4 class="sub">Details the assistant chooses</h4>
        <p class="mut">
          Anything that changes each time, such as which room. Use the same name in braces in the web address. Leave
          this empty if the address never changes.
        </p>
        {params.map((p) => (
          <div class="row paramrow" key={p.id}>
            <input
              type="text"
              maxLength={40}
              placeholder="room"
              aria-label="Detail name"
              autoCapitalize="none"
              value={p.name}
              onInput={(e) => edit(p.id, { name: e.currentTarget.value })}
            />
            <select
              aria-label="Kind of value"
              value={p.type}
              onChange={(e) => edit(p.id, { type: e.currentTarget.value })}
            >
              {TYPES.map(([v, label]) => (
                <option key={v} value={v}>
                  {label}
                </option>
              ))}
            </select>
            <label class="inline">
              <input
                type="checkbox"
                aria-label="Always needed"
                checked={p.required}
                onChange={(e) => edit(p.id, { required: e.currentTarget.checked })}
              />{" "}
              Always needed
            </label>
            <input
              type="text"
              maxLength={200}
              placeholder="What it means (optional)"
              aria-label="Description"
              value={p.description}
              onInput={(e) => edit(p.id, { description: e.currentTarget.value })}
            />
            <button class="plain small" type="button" onClick={() => setParams(params.filter((x) => x.id !== p.id))}>
              Remove
            </button>
          </div>
        ))}
        <div class="row">
          <AddButton
            label="Add a detail"
            id="adddetail"
            onClick={() =>
              setParams([...params, { id: nextParam++, name: "", type: "string", required: true, description: "" }])
            }
          />
        </div>
        <div class="row dlgfoot">
          <button>Add tool</button>
          <button class="plain" type="button" onClick={props.onClose}>
            Cancel
          </button>
        </div>
      </form>
    </Dialog>
  );
}

/** Every tool in one list, with a search and a way to narrow it to one source. */
export function Tools(): JSX.Element {
  const ext = useApp().ext;
  usePolled(loadExt);
  const [adding, setAdding] = useState(false);
  const [query, setQuery] = useState("");
  const [from, setFrom] = useState("");
  const all = ext?.tools ?? [];
  const rank = (t: Tool) => (t.source === "http" ? 0 : t.source === "mcp" ? 1 : 2);
  const sources = [...new Set(all.filter((t) => t.source === "mcp").map((t) => t.server ?? ""))].filter(Boolean);
  const inSource = (t: Tool) =>
    !from ||
    (from === "@added" ? t.source === "http" : from === "@builtin" ? t.source === "builtin" : t.server === from);
  const shown = matching(
    all.filter(inSource).sort((a, b) => rank(a) - rank(b)),
    query,
    (t) => t.description,
  );
  return (
    <>
      <section class="card" id="toolcard">
        <div class="head">
          <h3>
            Tools<span class="count">{all.length}</span>
          </h3>
          <AddButton label="Add a tool" id="addtool" onClick={() => setAdding(true)} />
        </div>
        <p class="mut">
          What the assistant can do besides talk. A tool that is off is never used. “Asks you first” means it reads the
          action back to you and waits for a yes.
        </p>
        {ext ? <Problems ext={ext} kind="tool" /> : null}
        <div class="toolbar">
          <input
            id="toolsearch"
            type="search"
            placeholder="Search tools"
            aria-label="Search tools"
            value={query}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />
          <select
            id="toolfrom"
            aria-label="Show tools from"
            value={from}
            onChange={(e) => setFrom(e.currentTarget.value)}
          >
            <option value="">All tools</option>
            <option value="@added">Added by you</option>
            <option value="@builtin">Built in</option>
            {sources.map((s) => (
              <option key={s} value={s}>
                From {s}
              </option>
            ))}
          </select>
        </div>
        {ext && !shown.length ? (
          <p class="empty">
            {all.length
              ? "No tool matches that."
              : "No tools yet. Use “Add a tool” to make one, or add a source above."}
          </p>
        ) : (
          <ToolList tools={shown} />
        )}
      </section>
      <AddTool open={adding} onClose={() => setAdding(false)} />
    </>
  );
}
