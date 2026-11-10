import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { isRefusal, refusal, speakBlock, type SpeakBlock } from "../dashboard.ts";
import { Field } from "../parts.tsx";
import { fail, load, say, useApp } from "../store.ts";

/** Say a phrase out loud, or say plainly why it cannot be done and where to fix that. Lives under the voice settings. */
export function TestSpeaker(): JSX.Element {
  const s = useApp();
  const [text, setText] = useState("");
  const [refused, setRefused] = useState<SpeakBlock | null>(null);
  const known = speakBlock(s.status);
  const block = known ?? refused;

  const speak = async (ev: Event) => {
    ev.preventDefault();
    setRefused(null);
    try {
      await api("POST", "/api/speak", { text });
      setText("");
      say("Sent. Listen for the speaker.");
      void load();
    } catch (x) {
      if (x instanceof ApiError && isRefusal(x.reason)) setRefused(refusal(x.reason));
      else fail(x);
    }
  };
  return (
    <form id="sayform" class="panel" onSubmit={speak}>
      <h3>Test the speaker</h3>
      <p class="mut">Type a sentence and the speaker says it out loud. Use it to check the voice and the volume.</p>
      <Field id="speaktext" label="What the speaker should say">
        <input
          id="speaktext"
          type="text"
          placeholder="Hello, is anyone home?"
          maxLength={200}
          disabled={!!known}
          value={text}
          onInput={(e) => setText(e.currentTarget.value)}
        />
      </Field>
      <div class="row">
        <button id="saybtn" disabled={!!known}>
          Say it
        </button>
      </div>
      {block ? (
        <p class="errtext" id="sayblock" role="status">
          {block.text}{" "}
          {block.to ? (
            <a href={block.to} id="sayfix">
              {block.label}
            </a>
          ) : null}
        </p>
      ) : null}
    </form>
  );
}
