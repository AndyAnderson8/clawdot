import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { useBusy } from "../parts.tsx";
import { api } from "../api.ts";
import { fail } from "../store.ts";
import { ask } from "../confirm.tsx";

/**
 * A key for a service, pasted once and kept on the device. The page only ever shows its first and last few characters.
 * Used where the key cannot be checked before it is kept (the speech and realtime services): a wrong key shows up the
 * first time the service is used, with a plain message.
 */
export function SavedKey(props: {
  /** For element ids: `<id>key`, `<id>hint`, `<id>save`, `<id>replace`, `<id>remove`, `<id>msg`. */
  id: string;
  label: string;
  what: string;
  /** Where to PUT `{key}` and DELETE, for example `/api/speech/key`. */
  path: string;
  /** The masked key the device reported, if one is stored. */
  hint: string | null | undefined;
  /** A key is in place some other way (an environment variable on the device). */
  fromEnv?: boolean;
  /** The controls cannot be used, and why. */
  off?: string;
  onChange?: (hint: string) => void;
}): JSX.Element {
  const [typed, setTyped] = useState("");
  const [replacing, setReplacing] = useState(false);
  const [busy, run] = useBusy();
  const [msg, setMsg] = useState("");
  const [bad, setBad] = useState(false);
  const [local, setLocal] = useState<string | null>(null);
  const hint = local ?? props.hint ?? "";
  const save = async () => {
    const key = typed.trim();
    if (!key) {
      setBad(true);
      setMsg("Paste the key first.");
      return;
    }
    await run(async () => {
      try {
        const r = await api<{ hint: string | null }>("PUT", props.path, { key });
        setLocal(r.hint ?? "");
        setTyped("");
        setReplacing(false);
        setBad(false);
        setMsg("Key saved on this device.");
        props.onChange?.(r.hint ?? "");
      } catch (x) {
        setBad(true);
        setMsg(x instanceof Error ? x.message : "The key could not be saved.");
      }
    });
  };
  const remove = async () => {
    if (!(await ask("Remove the saved key?", "Remove"))) return;
    try {
      await api("DELETE", props.path);
      setLocal("");
      setMsg("");
      props.onChange?.("");
    } catch (x) {
      fail(x);
    }
  };
  return (
    <div class="srow" id={props.id + "row"}>
      <div class="slabel">
        <label for={props.id + "key"}>{props.label}</label>
        <p class="mut">{props.what}</p>
      </div>
      <div class="sctl">
        {hint && !replacing ? (
          <div class="row keyshown">
            <code id={props.id + "hint"} class="keyhint">
              {hint}
            </code>
            <button class="plain small" type="button" id={props.id + "replace"} onClick={() => setReplacing(true)}>
              Replace
            </button>
            <button class="plain small" type="button" id={props.id + "remove"} onClick={() => void remove()}>
              Remove
            </button>
          </div>
        ) : (
          <div class="keyentry">
            {props.fromEnv && !hint ? (
              <p class="mut">Using the key from the environment variable on the device.</p>
            ) : null}
            <div class="row">
              <input
                id={props.id + "key"}
                type="password"
                autoComplete="off"
                placeholder={props.off ?? "Paste the key"}
                disabled={Boolean(props.off)}
                value={typed}
                onInput={(e) => setTyped(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void save();
                  }
                }}
              />
              <button
                class="plain small"
                type="button"
                id={props.id + "save"}
                disabled={busy || Boolean(props.off)}
                onClick={() => void save()}
              >
                {busy ? "Saving..." : "Save key"}
              </button>
              {replacing ? (
                <button class="plain small" type="button" onClick={() => setReplacing(false)}>
                  Cancel
                </button>
              ) : null}
            </div>
          </div>
        )}
        {msg ? (
          <p class={bad ? "errtext" : "mut"} role={bad ? "alert" : "status"} id={props.id + "msg"}>
            {msg}
          </p>
        ) : null}
      </div>
    </div>
  );
}
