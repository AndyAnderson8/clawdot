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
        <select id={props.id} value={props.value} onChange={(e) => props.onInput(e.currentTarget.value)} required>
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

/** A button that asks the language-model service what it offers, and fills the lists in the model boxes. */
export function ModelList(): JSX.Element {
  const [models, setModels] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const load = async () => {
    setBusy(true);
    try {
      const r = await api<{ models: string[] }>("GET", "/api/models");
      setModels(r.models);
      say(r.models.length + " models found. Click in a model box to choose one.");
    } catch (x) {
      fail(x);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="srow">
      <div class="slabel">
        <label for="loadmodels">What it offers</label>
        <p class="mut">Ask the service for its models, then choose from the list in the boxes below.</p>
      </div>
      <div class="sctl">
        <button class="plain small" type="button" id="loadmodels" disabled={busy} onClick={load}>
          {models.length ? "Ask again (" + models.length + " found)" : "See the models"}
        </button>
        <datalist id="models-list">
          {models.map((m) => (
            <option key={m} value={m} />
          ))}
        </datalist>
      </div>
    </div>
  );
}
