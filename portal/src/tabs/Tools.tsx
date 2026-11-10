import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { relaxWarning, tierLabel } from "../activity.ts";
import { Field } from "../parts.tsx";
import { change, loadExt, say, useApp, usePolled } from "../store.ts";
import { toolDefinition, type ParamForm } from "../forms.ts";
import type { Extensions, Tool } from "../types.ts";

/** The "Saved, but not running" box for tools or connections that failed to load. */
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

export function OnOffButton(props: { path: string; on: boolean; what: string }): JSX.Element {
  return (
    <button
      class="plain"
      type="button"
      onClick={() =>
        change("POST", props.path, { enabled: !props.on }, props.what + (props.on ? " turned off." : " turned on."))
      }
    >
      {props.on ? "Turn off" : "Turn on"}
    </button>
  );
}

/** The words in the "When it runs" column. */
function whenWords(tier: string): string {
  switch (tier) {
    case "read_only":
      return "Runs right away (only reads)";
    case "reversible":
      return "Runs right away (can be undone)";
    case "costly":
      return "Asks you out loud first";
    default:
      return tierLabel(tier);
  }
}

/** "When it runs": what the assistant does before using a tool (REQ-SEC-030). Only the ones you may change are a list. */
function WhenItRuns(props: { t: Tool }): JSX.Element {
  const { t } = props;
  const id = "tier-" + t.name;
  const pick = (to: string) => {
    const warn = relaxWarning(t.name, t.tier ?? "costly", to);
    if (warn && !confirm(warn)) return;
    void change(
      "POST",
      "/api/tools/" + t.name + "/tier",
      { tier: to === "" ? null : to },
      to === "" ? "Back to the usual level." : "Saved. It applies from the next request.",
    );
  };
  if (!(t.tier && t.tier_options?.length)) {
    // Not changeable here: say what it does, in the same words as the list.
    return (
      <span class={"when" + (t.confirm ? " ask" : "")}>
        {t.tier ? whenWords(t.tier) : t.confirm ? "Asks you out loud first" : "Runs right away"}
      </span>
    );
  }
  return (
    <div>
      <label class="sr" for={id}>
        When {t.name} runs
      </label>
      <select id={id} value={t.tier} onChange={(e) => pick(e.currentTarget.value)}>
        {(t.tier_options ?? []).map((o) => (
          <option key={o} value={o}>
            {whenWords(o)}
          </option>
        ))}
        {t.tier_set ? (
          <option value="">Back to the usual level ({tierLabel(t.tier_default ?? "costly")})</option>
        ) : null}
        <option value="destructive" disabled>
          Destructive: not available here
        </option>
      </select>
      {t.tier_set ? (
        <p class="mut tiernote">Changed here. The usual level is “{tierLabel(t.tier_default ?? "costly")}”.</p>
      ) : null}
    </div>
  );
}

/** Where a tool comes from, in a few words. */
function origin(t: Tool): string {
  if (t.source === "builtin") return "Built in";
  if (t.source === "mcp") return "From " + (t.server ?? "a connection");
  return t.origin === "config" ? "Your configuration file" : "Web tool you added";
}

function ToolRow(props: { t: Tool }) {
  const t = props.t;
  const remove = () => {
    if (confirm("Delete the tool " + t.name + "? The assistant will stop being able to use it."))
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
        ) : t.source === "http" ? (
          <span class="mut" title="Delete it from the configuration file to remove it.">
            In the file
          </span>
        ) : null}
      </div>
    </li>
  );
}

/** A list of tools with labelled columns, so each thing on a row says what it is. */
export function ToolList(props: { tools: Tool[] }): JSX.Element {
  return (
    <>
      <div class="toolhead" aria-hidden="true">
        <span>On</span>
        <span>Tool</span>
        <span>When it runs</span>
        <span />
      </div>
      <ul class="rows tools">
        {props.tools.map((t) => (
          <ToolRow key={t.name} t={t} />
        ))}
      </ul>
    </>
  );
}

const TYPES = ["string", "integer", "number", "boolean"];
const METHODS = ["GET", "POST", "PUT", "DELETE"];
let nextParam = 1;

function AddTool() {
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
    }
  };

  return (
    <form class="panel" onSubmit={submit}>
      <h3>Add a web tool</h3>
      <p class="mut">
        The assistant can call one fixed web address. It can only fill in the values you list, and it cannot change the
        address or the token.
      </p>
      <Field id="tool-name" label="Name" hint="Lowercase letters, digits, and underscores.">
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
      <Field id="tool-method" label="Method">
        <select id="tool-method" value={method} onChange={(e) => setMethod(e.currentTarget.value)}>
          {METHODS.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
      </Field>
      <Field
        id="tool-url"
        label="Web address"
        hint="Put a value's name in braces where it goes, for example {room}. Values in the address must be listed below as required."
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
      <Field
        id="tool-token"
        label="Token variable (optional)"
        hint="The name of an environment variable on the device that holds the token. Type the name only, never the token itself."
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
      <h4 class="sub">Values the assistant fills in</h4>
      <div>
        {params.map((p) => (
          <div class="row" key={p.id}>
            <input
              type="text"
              maxLength={40}
              placeholder="room"
              aria-label="Value name"
              autoCapitalize="none"
              value={p.name}
              onInput={(e) => edit(p.id, { name: e.currentTarget.value })}
            />
            <select aria-label="Type" value={p.type} onChange={(e) => edit(p.id, { type: e.currentTarget.value })}>
              {TYPES.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
            <label class="inline">
              <input
                type="checkbox"
                aria-label="Required"
                checked={p.required}
                onChange={(e) => edit(p.id, { required: e.currentTarget.checked })}
              />{" "}
              Required
            </label>
            <input
              type="text"
              maxLength={200}
              placeholder="What it means (optional)"
              aria-label="Description"
              value={p.description}
              onInput={(e) => edit(p.id, { description: e.currentTarget.value })}
            />
            <button class="plain" type="button" onClick={() => setParams(params.filter((x) => x.id !== p.id))}>
              Remove
            </button>
          </div>
        ))}
      </div>
      <button
        class="plain"
        type="button"
        onClick={() =>
          setParams([...params, { id: nextParam++, name: "", type: "string", required: true, description: "" }])
        }
      >
        Add a value the assistant fills in
      </button>
      <div class="row">
        <button>Add tool</button>
      </div>
    </form>
  );
}

export function Tools(): JSX.Element {
  const ext = useApp().ext;
  usePolled(loadExt);
  const by = (s: string) => (ext?.tools ?? []).filter((t) => t.source === s);
  const web = by("http");
  const conn = by("mcp");
  const own = by("builtin");
  return (
    <>
      <section class="card">
        <h3>
          Tools<span class="count">{(ext?.tools ?? []).length}</span>
        </h3>
        <p class="mut">
          Everything the assistant can do besides talk. Switch a tool off and it is never used. The column “When it
          runs” is whether the assistant asks you out loud before it does something.
        </p>
        <details id="tier-help" class="help">
          <summary>What does “When it runs” mean?</summary>
          <p>
            Every tool has a level. A tool that asks you out loud first is read back to you and runs only when you say
            yes. One that “can be undone” or “only reads” runs right away. Web tools and connections start at asking
            first, and you can relax one only for a tool you trust. A built-in tool can only be made stricter. Nothing
            can be made destructive here.
          </p>
        </details>
        <div>
          {ext ? (
            <>
              <Problems ext={ext} kind="tool" />
              <h4 class="sub">
                Web tools<span class="count">{web.length}</span>
              </h4>
              {web.length ? (
                <ToolList tools={web} />
              ) : (
                <p class="mut">
                  No web tools yet. A web tool lets the assistant call a web address, for example to switch on a light.
                  Add one below.
                </p>
              )}
              <h4 class="sub">
                From connections<span class="count">{conn.length}</span>
              </h4>
              {conn.length ? (
                <ToolList tools={conn} />
              ) : (
                <p class="mut">None yet. Tools from outside servers appear here once you add a connection.</p>
              )}
              <details id="builtin-tools">
                <summary>
                  Built in<span class="count">{own.length}</span>
                  <span class="mut">what the assistant can already do</span>
                </summary>
                <ToolList tools={own} />
              </details>
            </>
          ) : null}
        </div>
      </section>
      <AddTool />
    </>
  );
}
