import type { JSX } from "preact";
import { go } from "../parts.tsx";
import { useApp } from "../store.ts";
import { ago } from "../time.ts";

const STATE: Record<string, string> = {
  new: "Waiting to be read out",
  announced: "Read out",
  acknowledged: "You handled it",
  dismissed: "Dismissed",
  snoozed: "Snoozed",
};

export function Agents(): JSX.Element {
  const ov = useApp().overview;
  return (
    <>
      <h2>Agents</h2>
      <p>
        News from your coding agents, like Claude Code, such as “the build finished” or “I need your answer”. The
        speaker reads it out when it is idle.
      </p>
      <ul class="rows">
        {ov && !ov.events.length ? (
          <li>
            <p>Nothing from your agents yet.</p>
            <button type="button" onClick={() => go("webhooks")}>
              Connect an agent
            </button>
          </li>
        ) : null}
        {ov?.events.map((e) => (
          <li key={e.id}>
            <div class="rowhead">
              <strong>{e.title}</strong>
              <span class="badge">{STATE[e.state] ?? e.state}</span>
            </div>
            <div class="mut">
              {e.source} through the “{e.endpoint}” webhook, {ago(e.received_at, ov.now)}
            </div>
            {e.text ? <div>{e.text}</div> : null}
          </li>
        ))}
      </ul>
    </>
  );
}
