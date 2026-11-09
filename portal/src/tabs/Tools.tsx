import { useState } from "preact/hooks";
import { ConfirmBadge, Field, OffBadge } from "../parts.tsx";
import { change, loadExt, say, useApp, usePolled } from "../store.ts";
import { toolDefinition, type ParamForm } from "../forms.ts";
import type { Extensions, Tool } from "../types.ts";

/** The "Saved, but not running" box for tools or connections that failed to load. */
export function Problems(props: { ext: Extensions; kind: string }) {
  const list = (props.ext.problems ?? []).filter((x) => x.kind === props.kind);
  if (!list.length) return null;
  return (
    <div class="soon">
      <h3>Saved, but not running</h3>
      {list.map((x) => <p key={x.name}>{x.name}: {x.reason}. Fix that and restart ClawDot.</p>)}
    </div>
  );
}

export function OnOffButton(props: { path: string; on: boolean; what: string }) {
  return (
    <button class="plain" type="button" onClick={() => change("POST", props.path, { enabled: !props.on }, props.what + (props.on ? " turned off." : " turned on."))}>
      {props.on ? "Turn off" : "Turn on"}
    </button>
  );
}

function ToolRow(props: { t: Tool }) {
  const t = props.t;
  const remove = () => {
    if (confirm("Delete the tool " + t.name + "? The assistant will stop being able to use it.")) void change("DELETE", "/api/tools/" + t.name, undefined, "Deleted.");
  };
  return (
    <li>
      <div class="rowhead">
        <strong>{t.name}</strong>
        <span><OffBadge enabled={t.enabled} /> <ConfirmBadge confirm={t.confirm} /></span>
      </div>
      <div class="mut">{t.description}</div>
      {t.source === "http" ? <div class="mut">{t.origin === "config" ? "From your configuration file." : "Added here."}</div> : null}
      {t.source === "mcp" ? <div class="mut">From the connection {t.server}.</div> : null}
      <div class="acts">
        <OnOffButton path={"/api/tools/" + t.name + "/enabled"} on={t.enabled} what={t.name} />
        {t.deletable ? <button class="danger" type="button" onClick={remove}>Delete</button>
          : t.source === "http" ? <span class="mut">To remove it, delete it from the configuration file.</span> : null}
      </div>
    </li>
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
  const edit = (id: number, patch: Partial<ParamForm>) => setParams(params.map((p) => (p.id === id ? { ...p, ...patch } : p)));

  const submit = async (ev: Event) => {
    ev.preventDefault();
    const def = toolDefinition({ name, description, method, url, tokenVar, confirm: confirmFirst, params });
    if (typeof def === "string") { say(def, true); return; }
    if (await change("POST", "/api/tools", def, "Added. The assistant can use it now.")) {
      setName(""); setDescription(""); setMethod("POST"); setUrl(""); setTokenVar(""); setConfirmFirst(true); setParams([]);
    }
  };

  return (
    <form class="panel" onSubmit={submit}>
      <h3>Add a web tool</h3>
      <p class="mut">The assistant can call one fixed web address. It can only fill in the values you list, and it cannot change the address or the token.</p>
      <Field id="tool-name" label="Name" hint="Lowercase letters, digits, and underscores.">
        <input id="tool-name" type="text" maxLength={40} autoCapitalize="none" autoComplete="off" placeholder="set_light" value={name} onInput={(e) => setName(e.currentTarget.value)} />
      </Field>
      <Field id="tool-desc" label="What it does" hint="The assistant reads this to decide when to use it.">
        <input id="tool-desc" type="text" maxLength={300} placeholder="Turn a room's light on or off" value={description} onInput={(e) => setDescription(e.currentTarget.value)} />
      </Field>
      <Field id="tool-method" label="Method">
        <select id="tool-method" value={method} onChange={(e) => setMethod(e.currentTarget.value)}>
          {METHODS.map((m) => <option key={m} value={m}>{m}</option>)}
        </select>
      </Field>
      <Field id="tool-url" label="Web address" hint="Put a value's name in braces where it goes, for example {room}. Values in the address must be listed below as required.">
        <input id="tool-url" type="text" inputMode="url" autoCapitalize="none" autoComplete="off" placeholder="https://home.local/api/light/{room}" value={url} onInput={(e) => setUrl(e.currentTarget.value)} />
      </Field>
      <Field id="tool-token" label="Token variable (optional)" hint="The name of an environment variable on the device that holds the token. Type the name only, never the token itself.">
        <input id="tool-token" type="text" maxLength={64} autoCapitalize="none" autoComplete="off" placeholder="HA_TOKEN" value={tokenVar} onInput={(e) => setTokenVar(e.currentTarget.value)} />
      </Field>
      <div class="row">
        <input type="checkbox" id="tool-asks" checked={confirmFirst} onChange={(e) => setConfirmFirst(e.currentTarget.checked)} />
        <label for="tool-asks" class="inline">Ask me out loud before it runs (recommended)</label>
      </div>
      <h3>Values the assistant fills in</h3>
      <div>
        {params.map((p) => (
          <div class="row" key={p.id}>
            <input type="text" maxLength={40} placeholder="room" aria-label="Value name" autoCapitalize="none" value={p.name} onInput={(e) => edit(p.id, { name: e.currentTarget.value })} />
            <select aria-label="Type" value={p.type} onChange={(e) => edit(p.id, { type: e.currentTarget.value })}>
              {TYPES.map((m) => <option key={m} value={m}>{m}</option>)}
            </select>
            <label class="inline"><input type="checkbox" aria-label="Required" checked={p.required} onChange={(e) => edit(p.id, { required: e.currentTarget.checked })} /> Required</label>
            <input type="text" maxLength={200} placeholder="What it means (optional)" aria-label="Description" value={p.description} onInput={(e) => edit(p.id, { description: e.currentTarget.value })} />
            <button class="plain" type="button" onClick={() => setParams(params.filter((x) => x.id !== p.id))}>Remove</button>
          </div>
        ))}
      </div>
      <button class="plain" type="button" onClick={() => setParams([...params, { id: nextParam++, name: "", type: "string", required: true, description: "" }])}>
        Add a value the assistant fills in
      </button>
      <div class="row"><button>Add tool</button></div>
    </form>
  );
}

export function Tools() {
  const ext = useApp().ext;
  usePolled(loadExt);
  const by = (s: string) => (ext?.tools ?? []).filter((t) => t.source === s);
  const web = by("http");
  const conn = by("mcp");
  const own = by("builtin");
  return (
    <>
      <h2>Tools</h2>
      <p>Everything the assistant can do besides talk. Turn any tool off or on. Tools that change things in the outside world ask you out loud first.</p>
      <div>
        {ext ? (
          <>
            <Problems ext={ext} kind="tool" />
            <h3>Web tools ({web.length})</h3>
            {web.length ? <ul class="rows">{web.map((t) => <ToolRow key={t.name} t={t} />)}</ul>
              : <p class="mut">No web tools yet. A web tool lets the assistant call a web address, for example to switch on a light. Add one below.</p>}
            <h3>From connections ({conn.length})</h3>
            {conn.length ? <ul class="rows">{conn.map((t) => <ToolRow key={t.name} t={t} />)}</ul>
              : <p class="mut">None yet. Tools from outside servers appear here once you add a connection.</p>}
            <details id="builtin-tools">
              <summary>Built in ({own.length}): what the assistant can already do</summary>
              <ul class="rows">{own.map((t) => <ToolRow key={t.name} t={t} />)}</ul>
            </details>
          </>
        ) : null}
      </div>
      <AddTool />
    </>
  );
}
