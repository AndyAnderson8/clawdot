import { useState } from "preact/hooks";
import { api } from "../api.ts";
import { snippets } from "../forms.ts";
import { CopyButton } from "../parts.tsx";
import { fail, get, load, say, set, sv, useApp } from "../store.ts";
import type { Endpoint, WebhookToken } from "../types.ts";

const base = () => String(sv("public_url") || location.origin);

function Hook(props: { e: Endpoint; open: boolean; setOpen: (v: boolean) => void }) {
  const { e } = props;
  const tok = useApp().fresh[e.name];
  const url = base() + "/hook/" + e.name;

  const rotate = async () => {
    if (!confirm("Make a new token for " + e.name + "? The old one stops working, so update whatever uses it.")) return;
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
    if (!confirm("Delete the webhook " + e.name + "? Anything posting to it will be turned away.")) return;
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
        <span class="badge">{e.adapter === "claude-code" ? "Claude Code" : "Plain JSON"}</span>
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

export function Webhooks() {
  const s = useApp();
  const [name, setName] = useState("");
  const [adapter, setAdapter] = useState("claude-code");
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const ov = s.overview;

  const create = async (ev: Event) => {
    ev.preventDefault();
    try {
      const r = await api<WebhookToken>("POST", "/api/endpoints", { name: name.trim(), adapter });
      set({ fresh: { ...get().fresh, [r.name]: r.token } });
      setName("");
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
      <h2>Webhooks</h2>
      <p>
        A webhook is a web address that an agent or service posts to when something happens. Each has its own token,
        which works like a password for that address.
      </p>
      <ul class="rows">
        {ov && !ov.endpoints.length ? (
          <li class="mut">No webhooks yet. Create one below to connect your first agent.</li>
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
      <form class="panel" onSubmit={create}>
        <h3>Create a webhook</h3>
        <p class="mut">
          Give each agent or service its own webhook so you can cut one off without touching the rest. Use letters,
          numbers, - and _.
        </p>
        <label for="wname">Name</label>
        <input
          id="wname"
          type="text"
          maxLength={32}
          placeholder="For example: claude-code"
          autoComplete="off"
          autoCapitalize="none"
          value={name}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <label for="wadapter">What is sending?</label>
        <select id="wadapter" value={adapter} onChange={(e) => setAdapter(e.currentTarget.value)}>
          <option value="claude-code">Claude Code</option>
          <option value="generic">Anything else (plain JSON)</option>
        </select>
        <div class="row">
          <button>Create webhook</button>
        </div>
      </form>
    </>
  );
}
