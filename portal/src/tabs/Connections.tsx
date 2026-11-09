import { useState } from "preact/hooks";
import { api } from "../api.ts";
import { connectionDefinition } from "../forms.ts";
import { ConfirmBadge, Field, OffBadge } from "../parts.tsx";
import { change, fail, loadExt, useApp, usePolled } from "../store.ts";
import type { ConnectionTest, McpServer } from "../types.ts";
import { OnOffButton, Problems } from "./Tools.tsx";

function Server(props: { s: McpServer }) {
  const s = props.s;
  const remove = () => {
    if (confirm("Remove the connection " + s.name + "? Its tools will disappear.")) void change("DELETE", "/api/connections/" + s.name, undefined, "Removed.");
  };
  return (
    <li>
      <div class="rowhead">
        <strong>{s.name}</strong>
        <span><OffBadge enabled={s.enabled} /> <span class={"badge " + (s.connected ? "good" : "bad")}>{s.connected ? "Connected" : "Cannot connect"}</span></span>
      </div>
      <div class="mut">{s.url + (s.origin === "config" ? " (from your configuration file)" : "")}</div>
      {!s.connected ? <p>It did not answer: {s.error || "unknown reason"}. Check the address, that the server is running, and that the token variable is set, then restart ClawDot.</p> : null}
      {s.token_env ? <div class="mut">Token comes from the environment variable {s.token_env}.</div> : null}
      {s.tools.length ? (
        <details id={"tools-" + s.name}>
          <summary>{s.tools.length + (s.tools.length === 1 ? " tool" : " tools")}</summary>
          <ul class="rows">
            {s.tools.map((t) => (
              <li key={t.name}>
                <div class="rowhead"><strong>{t.name}</strong><span><OffBadge enabled={t.enabled} /> <ConfirmBadge confirm={t.confirm} /></span></div>
                <div class="mut">{t.description}</div>
                <div class="acts"><OnOffButton path={"/api/tools/" + t.name + "/enabled"} on={t.enabled} what={t.name} /></div>
              </li>
            ))}
          </ul>
        </details>
      ) : s.connected ? <p class="mut">Connected, but it offers no tools.</p> : null}
      <div class="acts">
        <OnOffButton path={"/api/connections/" + s.name + "/enabled"} on={s.enabled} what={s.name} />
        {s.deletable ? <button class="danger" type="button" onClick={remove}>Remove</button> : <span class="mut">To remove it, delete it from the configuration file.</span>}
      </div>
    </li>
  );
}

function AddConnection() {
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [tokenVar, setTokenVar] = useState("");
  const [confirmFirst, setConfirmFirst] = useState(true);
  const [result, setResult] = useState("");
  const def = () => connectionDefinition({ name, url, tokenVar, confirm: confirmFirst });

  const test = async () => {
    setResult("Connecting...");
    try {
      const r = await api<ConnectionTest>("POST", "/api/connections/test", def());
      const n = r.tools.length;
      setResult("Connected. It offers " + n + (n === 1 ? " tool" : " tools") + (n ? ": " + r.tools.map((t) => t.name).join(", ") : "") + ".");
    } catch (x) { setResult(""); fail(x); }
  };
  const submit = async (ev: Event) => {
    ev.preventDefault();
    setResult("Connecting...");
    const ok = await change("POST", "/api/connections", def(), "Connected and added.");
    setResult("");
    if (ok) { setName(""); setUrl(""); setTokenVar(""); setConfirmFirst(true); }
  };

  return (
    <form class="panel" onSubmit={submit}>
      <h3>Add a connection</h3>
      <p class="mut">The server must speak MCP over HTTP. It is tested before it is saved.</p>
      <Field id="conn-name" label="Name" hint="Lowercase letters, digits, - and _. Its tools start with this name.">
        <input id="conn-name" type="text" maxLength={24} autoCapitalize="none" autoComplete="off" placeholder="home" value={name} onInput={(e) => setName(e.currentTarget.value)} />
      </Field>
      <Field id="conn-url" label="Server address">
        <input id="conn-url" type="text" inputMode="url" autoCapitalize="none" autoComplete="off" placeholder="https://homeassistant.local:8123/mcp" value={url} onInput={(e) => setUrl(e.currentTarget.value)} />
      </Field>
      <Field id="conn-token" label="Token variable (optional)" hint="The name of an environment variable on the device that holds the token. Type the name only, never the token itself.">
        <input id="conn-token" type="text" maxLength={64} autoCapitalize="none" autoComplete="off" placeholder="HA_TOKEN" value={tokenVar} onInput={(e) => setTokenVar(e.currentTarget.value)} />
      </Field>
      <div class="row">
        <input type="checkbox" id="conn-asks" checked={confirmFirst} onChange={(e) => setConfirmFirst(e.currentTarget.checked)} />
        <label for="conn-asks" class="inline">Ask me out loud before its tools run (recommended)</label>
      </div>
      <div class="row"><button>Add connection</button><button class="plain" type="button" onClick={test}>Test connection</button></div>
      <p role="status">{result}</p>
    </form>
  );
}

export function Connections() {
  const ext = useApp().ext;
  usePolled(loadExt);
  const servers = ext?.mcp ?? [];
  return (
    <>
      <h2>Connections</h2>
      <p>A connection links ClawDot to an outside tool server using MCP (the Model Context Protocol), such as Home Assistant or a calendar. Its tools then show up under Tools, where you can turn each one off.</p>
      <div>
        {ext ? (
          <>
            <Problems ext={ext} kind="connection" />
            {!servers.length ? <p>No connections yet. Add one below.</p> : <ul class="rows">{servers.map((s) => <Server key={s.name} s={s} />)}</ul>}
          </>
        ) : null}
      </div>
      <AddConnection />
    </>
  );
}
