import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, enc } from "../api.ts";
import { AddButton, Dialog } from "../dialog.tsx";
import { connectionDefinition } from "../forms.ts";
import { Field, useBusy } from "../parts.tsx";
import { change, fail, loadExt, say, useApp, usePolled } from "../store.ts";
import type { ConnectionTest, McpServer } from "../types.ts";
import { Problems } from "./Tools.tsx";
import { ask } from "../confirm.tsx";

function Source(props: { s: McpServer }) {
  const s = props.s;
  const remove = async () => {
    if (await ask("Remove the source " + s.name + "? Its tools will disappear.", "Remove"))
      void change("DELETE", "/api/connections/" + enc(s.name), undefined, "Removed.");
  };
  const n = s.tools.length;
  const waiting = s.needs_review ?? [];
  const accept = (tool?: string) =>
    change(
      "POST",
      "/api/connections/" + enc(s.name) + "/accept",
      tool ? { tool } : {},
      tool ? "Accepted." : "All accepted.",
    );
  return (
    <li class={"toolrow srcrow" + (s.enabled ? "" : " off")}>
      <input
        class="switch"
        type="checkbox"
        checked={s.enabled}
        aria-label={s.name + (s.enabled ? " is on" : " is off")}
        onChange={() =>
          change(
            "POST",
            "/api/connections/" + enc(s.name) + "/enabled",
            { enabled: !s.enabled },
            s.name + (s.enabled ? " turned off." : " turned on."),
          )
        }
      />
      <div class="toolmain">
        <div class="toolname">{s.name}</div>
        <div class="tooldesc">{s.url}</div>
        {!s.connected ? (
          <div class="tooldesc bad">
            Cannot connect: {s.error || "unknown reason"}. Check the address and token variable, then restart ClawDot.
          </div>
        ) : null}
        {waiting.length ? (
          <div class="review" id={"review-" + s.name}>
            <p>
              <span class="badge warn">Needs review</span>{" "}
              {waiting.length === 1 ? "One tool is" : waiting.length + " tools are"} new or changed since you approved
              this source. The assistant cannot use {waiting.length === 1 ? "it" : "them"} until you accept. What a tool
              says about itself is read by the assistant, so read it first.
            </p>
            <ul class="reviewlist">
              {waiting.map((t) => (
                <li key={t.name}>
                  <div class="toolname">{t.name}</div>
                  <div class="tooldesc">{t.reason_text}</div>
                  <pre class="skilltext">{t.description || "It gives no description."}</pre>
                  {t.arguments.length ? <div class="tooldesc">It takes: {t.arguments.join(", ")}</div> : null}
                  <button class="plain small" type="button" onClick={() => void accept(t.name)}>
                    Accept this tool
                  </button>
                </li>
              ))}
            </ul>
            {waiting.length > 1 ? (
              <button class="plain small" type="button" onClick={() => void accept()}>
                Accept all {waiting.length}
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
      <div class="toolwhen">
        <span class={"badge " + (s.connected ? "good" : "bad")}>{s.connected ? "Connected" : "Cannot connect"}</span>{" "}
        <span class="mut">{n + (n === 1 ? " tool" : " tools")}</span>
      </div>
      <div class="toolact">
        {s.deletable ? (
          <button class="danger small" type="button" onClick={remove}>
            Remove
          </button>
        ) : (
          <span class="mut">Set when ClawDot was installed</span>
        )}
      </div>
    </li>
  );
}

/** Add a source: check it, choose which of its tools to use, add it. */
function AddSource(props: { open: boolean; onClose: () => void }) {
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [tokenVar, setTokenVar] = useState("");
  const [confirmFirst, setConfirmFirst] = useState(true);
  const [found, setFound] = useState<string[] | null>(null);
  const [off, setOff] = useState<string[]>([]);
  const [busy, run] = useBusy();
  const def = () => connectionDefinition({ name, url, tokenVar, confirm: confirmFirst });
  const changed =
    <T,>(set: (v: T) => void) =>
    (v: T) => {
      set(v);
      setFound(null);
      setOff([]);
    };

  const check = async () => {
    try {
      const r = await api<ConnectionTest>("POST", "/api/connections/test", def());
      setFound(r.tools.map((t) => t.name));
      setOff([]);
    } catch (x) {
      fail(x);
    }
  };
  const add = async () => {
    const ok = await change("POST", "/api/connections", def(), "");
    if (ok) {
      // Every tool comes in; the ones left unticked are switched off.
      for (const t of off) {
        try {
          await api("POST", "/api/tools/" + enc(t) + "/enabled", { enabled: false });
        } catch (x) {
          fail(x);
        }
      }
      await loadExt();
      say(found ? "Added. " + (found.length - off.length) + " of its " + found.length + " tools are on." : "Added.");
      setName("");
      setUrl("");
      setTokenVar("");
      setConfirmFirst(true);
      setFound(null);
      setOff([]);
      props.onClose();
    }
  };
  const submit = (ev: Event) => {
    ev.preventDefault();
    void run(found ? add : check);
  };

  return (
    <Dialog id="sourcedialog" title="Add a source" open={props.open} onClose={props.onClose} wide>
      <form class="dlgform" onSubmit={submit}>
        <p class="mut">
          A source is a server that offers tools to the assistant, such as a calendar or a smart-home hub. It must speak
          MCP over HTTP. ClawDot connects first, so you can choose which of its tools to use.
        </p>
        <Field id="conn-name" label="Name" hint="Lowercase letters, digits, - and _. Its tools start with this name.">
          <input
            id="conn-name"
            type="text"
            maxLength={24}
            autoCapitalize="none"
            autoComplete="off"
            placeholder="home"
            value={name}
            onInput={(e) => changed(setName)(e.currentTarget.value)}
          />
        </Field>
        <Field id="conn-url" label="Address">
          <input
            id="conn-url"
            type="text"
            inputMode="url"
            autoCapitalize="none"
            autoComplete="off"
            placeholder="https://homeassistant.local:8123/mcp"
            value={url}
            onInput={(e) => changed(setUrl)(e.currentTarget.value)}
          />
        </Field>
        <Field
          id="conn-token"
          label="Token variable"
          hint="Optional. The name of an environment variable on the device that holds the source's token. Type the name only, never the token itself."
        >
          <input
            id="conn-token"
            type="text"
            maxLength={64}
            autoCapitalize="none"
            autoComplete="off"
            placeholder="HA_TOKEN"
            value={tokenVar}
            onInput={(e) => changed(setTokenVar)(e.currentTarget.value)}
          />
        </Field>
        <div class="row">
          <input
            type="checkbox"
            id="conn-asks"
            checked={confirmFirst}
            onChange={(e) => setConfirmFirst(e.currentTarget.checked)}
          />
          <label for="conn-asks" class="inline">
            Ask me out loud before its tools run (recommended)
          </label>
        </div>
        {found ? (
          <fieldset class="pickbox" id="sourcetools">
            <legend>
              Found {found.length} {found.length === 1 ? "tool" : "tools"}
            </legend>
            <p class="mut">
              All of them are added. Untick any you do not want the assistant to use; you can turn them on later.
            </p>
            {found.length ? (
              <>
                <div class="row">
                  <button class="plain small" type="button" onClick={() => setOff([])}>
                    Select all
                  </button>
                  <button class="plain small" type="button" onClick={() => setOff(found)}>
                    Select none
                  </button>
                  <span class="mut" role="status">
                    {found.length - off.length} of {found.length} selected
                  </span>
                </div>
                <ul class="picklist">
                  {found.map((t) => (
                    <li key={t}>
                      <label class="inline">
                        <input
                          type="checkbox"
                          checked={!off.includes(t)}
                          onChange={(e) => setOff(e.currentTarget.checked ? off.filter((x) => x !== t) : [...off, t])}
                        />{" "}
                        <span class="toolname">{t}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              </>
            ) : (
              <p class="mut">It connected, but it offers no tools.</p>
            )}
          </fieldset>
        ) : null}
        <div class="row dlgfoot">
          <button disabled={busy}>{busy ? "Connecting..." : found ? "Add source" : "Connect and see its tools"}</button>
          <button class="plain" type="button" onClick={props.onClose}>
            Cancel
          </button>
        </div>
      </form>
    </Dialog>
  );
}

/** The outside servers that provide tools. Their tools are in the Tools list. */
export function Sources(): JSX.Element {
  const ext = useApp().ext;
  usePolled(loadExt);
  const [adding, setAdding] = useState(false);
  const servers = ext?.mcp ?? [];
  return (
    <>
      <section class="card" id="sourcecard">
        <div class="head">
          <h3>
            Tool sources<span class="count">{servers.length}</span>
          </h3>
          <AddButton label="Add a source" id="addsource" onClick={() => setAdding(true)} />
        </div>
        <p class="mut">
          Servers that give the assistant more tools, such as a calendar or a smart-home hub. What they offer shows up
          in the list below.
        </p>
        {ext ? <Problems ext={ext} kind="connection" /> : null}
        {ext && !servers.length ? (
          <p class="empty">No sources yet. Add one to bring in tools from another program.</p>
        ) : (
          <ul class="rows tools">
            {servers.map((s) => (
              <Source key={s.name} s={s} />
            ))}
          </ul>
        )}
      </section>
      <AddSource open={adding} onClose={() => setAdding(false)} />
    </>
  );
}
