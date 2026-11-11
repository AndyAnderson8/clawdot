import type { JSX } from "preact";
import { Field } from "../parts.tsx";
import { AddButton, Dialog } from "../dialog.tsx";
import { useState } from "preact/hooks";
import { api } from "../api.ts";
import { ADAPTERS, adapterName, snippets } from "../snippets.ts";
import { CopyButton } from "../parts.tsx";
import { fail, get, load, say, set, sv, useApp } from "../store.ts";
import type { Endpoint, WebhookToken } from "../types.ts";

const base = () => String(sv("public_url") || location.origin);

function Hook(props: { e: Endpoint; open: boolean; setOpen: (v: boolean) => void }) {
  const { e } = props;
  const tok = useApp().fresh[e.name];
  const url = base() + "/hook/" + e.name;

  const rotate = async () => {
    if (!confirm("Make a new token for " + e.name + "? The old one stops working, so update the program that uses it."))
      return;
    try {
      const r = await api<WebhookToken>("POST", "/api/endpoints/" + e.name + "/rotate");
      set({ fresh: { ...get().fresh, [e.name]: r.token } });
      say("New token made. Copy it now; it is shown once.");
      props.setOpen(true);
      await load();
    } catch (x) {
      fail(x);
    }
  };
  const remove = async () => {
    if (!confirm("Remove the webhook " + e.name + "? Anything sending to it will be turned away.")) return;
    try {
      await api("DELETE", "/api/endpoints/" + e.name);
      const { [e.name]: _gone, ...rest } = get().fresh;
      set({ fresh: rest });
      say("Deleted.");
      await load();
    } catch (x) {
      fail(x);
    }
  };

  return (
    <li>
      <div class="rowhead">
        <strong>{e.name}</strong>
        <span class="badge">{adapterName(e.adapter)}</span>
      </div>
      <code>{url}</code>
      <div class="acts">
        <CopyButton text={url} label="Copy address" />
        <button class="plain" type="button" onClick={rotate}>
          New token
        </button>
        <button class="danger" type="button" onClick={remove}>
          Delete
        </button>
      </div>
      <details id={"snip-" + e.name} open={props.open} onToggle={(ev) => props.setOpen(ev.currentTarget.open)}>
        <summary>Setup and snippets</summary>
        {tok ? (
          <>
            <p>Your token (shown once, copy it now):</p>
            <code class="token">{tok}</code>
            <CopyButton text={tok} label="Copy token" />
          </>
        ) : (
          <p class="mut">
            The token is not shown again. The snippets below use YOUR_TOKEN; replace it with the token you saved, or
            make a new token.
          </p>
        )}
        {snippets(base(), e.name, e.adapter, tok).map(([title, text]) => (
          <div key={title}>
            <h3>{title}</h3>
            <pre>{text}</pre>
            <CopyButton text={text} />
          </div>
        ))}
      </details>
    </li>
  );
}

export function Webhooks(): JSX.Element {
  const s = useApp();
  const [name, setName] = useState("");
  const [adding, setAdding] = useState(false);
  const [adapter, setAdapter] = useState("generic");
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const ov = s.overview;

  const create = async (ev: Event) => {
    ev.preventDefault();
    try {
      const r = await api<WebhookToken>("POST", "/api/endpoints", { name: name.trim(), adapter });
      set({ fresh: { ...get().fresh, [r.name]: r.token } });
      setName("");
      setAdding(false);
      setOpen((o) => ({ ...o, [r.name]: true }));
      say("Created. Copy the token now; it is shown once.");
      await load();
      document.getElementById("snip-" + r.name)?.scrollIntoView({ block: "center" });
    } catch (x) {
      fail(x);
    }
  };

  return (
    <>
      <p class="lead">
        A webhook is an address other programs post a message to, so they can speak through your speaker: a build server
        that has finished, a home automation hub, a script. Each webhook gets its own address and token, which works
        like a password. When a message arrives, the speaker reads it out as soon as it is idle.
      </p>
      <section class="card" id="inputlist">
        <div class="head">
          <h3>
            Webhooks<span class="count">{ov?.endpoints.length ?? 0}</span>
          </h3>
          <AddButton label="Add a webhook" id="addwebhook" onClick={() => setAdding(true)} />
        </div>
        <ul class="rows">
          {ov && !ov.endpoints.length ? (
            <li class="empty">
              No webhooks yet. Add one, then paste its address and token into the program that should send messages.
            </li>
          ) : null}
          {ov?.endpoints.map((e) => (
            <Hook
              key={e.name}
              e={e}
              open={open[e.name] ?? !!s.fresh[e.name]}
              setOpen={(v) => setOpen((o) => ({ ...o, [e.name]: v }))}
            />
          ))}
        </ul>
      </section>
      <Dialog id="webhookdialog" title="Add a webhook" open={adding} onClose={() => setAdding(false)}>
        <form class="dlgform" onSubmit={create}>
          <p class="mut">
            Give each program its own webhook, so you can turn one off without touching the rest. Use letters, numbers,
            - and _.
          </p>
          <Field id="wname" label="Name">
            <input
              id="wname"
              type="text"
              maxLength={32}
              placeholder="For example: build-server"
              autoComplete="off"
              autoCapitalize="none"
              value={name}
              onInput={(e) => setName(e.currentTarget.value)}
            />
          </Field>
          <Field id="wadapter" label="Message format">
            <select id="wadapter" value={adapter} onChange={(e) => setAdapter(e.currentTarget.value)}>
              {ADAPTERS.map(([v, text]) => (
                <option key={v} value={v}>
                  {text}
                </option>
              ))}
            </select>
          </Field>
          <div class="row">
            <button>Add webhook</button>
          </div>
        </form>
      </Dialog>
    </>
  );
}
