import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api } from "../api.ts";
import { fail, say } from "../store.ts";

/** What the speaker says to let someone hear a voice. */
export const SAMPLE = "Hello. This is how I sound. I can set timers, answer questions and read you what comes in.";

/** The voice, chosen from what the speech server offers (a list when it has one), with a way to hear it. */
export function VoiceControl(props: {
  id: string;
  value: string;
  /** What is saved now: a sample plays the saved voice, so a changed choice must be saved first. */
  saved: string;
  off?: boolean;
  onInput: (v: string) => void;
}): JSX.Element {
  const [voices, setVoices] = useState<string[] | null>(null);
  const [why, setWhy] = useState("");
  useEffect(() => {
    if (props.off) return;
    api<{ voices: string[] }>("GET", "/api/voices")
      .then((r) => setVoices(r.voices))
      .catch((x) => {
        setVoices(null);
        setWhy(x instanceof Error ? x.message : "");
      });
  }, [props.off]);
  const sample = async () => {
    try {
      await api("POST", "/api/speak", { text: SAMPLE });
      say("Playing a sample. Listen for the speaker.");
    } catch (x) {
      fail(x);
    }
  };
  const unsaved = props.value !== props.saved;
  const list = voices ? (voices.includes(props.value) || !props.value ? voices : [props.value, ...voices]) : null;
  return (
    <div class="voicepick">
      {list ? (
        <select
          id={props.id}
          class={props.value ? "" : "unset"}
          value={props.value}
          onChange={(e) => props.onInput(e.currentTarget.value)}
        >
          {props.value ? null : (
            <option value="" disabled>
              Choose a voice
            </option>
          )}
          {list.map((v) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
      ) : (
        <input
          id={props.id}
          type="text"
          value={props.value}
          autoComplete="off"
          disabled={props.off}
          placeholder={props.off ? "Not available yet" : "The name of a voice"}
          onInput={(e) => props.onInput(e.currentTarget.value)}
        />
      )}
      {!props.off ? (
        <div class="row">
          <button class="plain small" type="button" id="hearvoice" disabled={unsaved || !props.value} onClick={sample}>
            Hear a sample
          </button>
          {unsaved && props.value ? <span class="mut">Save to hear this voice.</span> : null}
        </div>
      ) : null}
      {!list && why && !props.off ? <p class="mut">{why}</p> : null}
    </div>
  );
}

const known = new Map<string, Promise<string[]>>();

/** The models a service offers, asked once per address and key variable. */
function loadModels(url: string, keyEnv: string): Promise<string[]> {
  const k = url + "|" + keyEnv;
  let p = known.get(k);
  if (!p) {
    const q = "?url=" + encodeURIComponent(url) + (keyEnv ? "&key_env=" + encodeURIComponent(keyEnv) : "");
    p = api<{ models: string[] }>("GET", "/api/models" + q).then((r) => r.models);
    known.set(k, p);
    p.catch(() => known.delete(k));
  }
  return p;
}

/** The model, chosen from what the provider offers. Only if the provider has no list is it a box to type in. */
export function ModelSelect(props: {
  id: string;
  value: string;
  url: string;
  keyEnv: string;
  onInput: (v: string) => void;
}): JSX.Element {
  const [models, setModels] = useState<string[] | null>(null);
  const [why, setWhy] = useState("");
  const [busy, setBusy] = useState(false);
  const [again, setAgain] = useState(0);
  const askable = /^https?:\/\/[^/\s]{2,}/.test(props.url);
  useEffect(() => {
    setModels(null);
    setWhy("");
    if (!askable) return;
    let live = true;
    setBusy(true);
    loadModels(props.url, props.keyEnv)
      .then((m) => {
        if (live) setModels(m);
      })
      .catch((x) => {
        if (live) setWhy(x instanceof Error ? x.message : "The list could not be loaded.");
      })
      .finally(() => {
        if (live) setBusy(false);
      });
    return () => {
      live = false;
    };
  }, [props.url, props.keyEnv, askable, again]);
  if (busy) {
    return (
      <select id={props.id} disabled aria-busy="true">
        <option>Loading the models</option>
      </select>
    );
  }
  if (models) {
    const list = props.value && !models.includes(props.value) ? [props.value, ...models] : models;
    return (
      <select
        id={props.id}
        class={props.value ? "" : "unset"}
        value={props.value}
        onChange={(e) => props.onInput(e.currentTarget.value)}
      >
        {props.value ? null : (
          <option value="" disabled>
            Choose a model
          </option>
        )}
        {list.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>
    );
  }
  return (
    <div class="voicepick">
      <input
        id={props.id}
        type="text"
        value={props.value}
        autoComplete="off"
        placeholder="The name of a model"
        onInput={(e) => props.onInput(e.currentTarget.value)}
      />
      {askable ? (
        <p class="mut">
          {why ? why + " " : ""}Type the name of a model.{" "}
          <button class="plain small" type="button" onClick={() => setAgain(again + 1)}>
            Try again
          </button>
        </p>
      ) : (
        <p class="mut">Choose a provider and the models it offers appear here.</p>
      )}
    </div>
  );
}
