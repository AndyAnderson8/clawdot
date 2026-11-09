import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api } from "../api.ts";
import { go } from "../parts.tsx";
import { fail, load, say, set, useApp } from "../store.ts";

interface Item {
  title: string;
  done: boolean;
  why: string;
  action?: [tab: string, label: string];
}

export function Home(): JSX.Element {
  const s = useApp();
  const [text, setText] = useState("");
  const st = s.status;
  const ov = s.overview ?? { now: 0, endpoints: [], events: [] };

  const speak = async (ev: Event) => {
    ev.preventDefault();
    try {
      await api("POST", "/api/speak", { text });
      setText("");
      set({ spoke: true });
      say("Sent. Listen for the speaker.");
      void load();
    } catch (x) {
      fail(x);
    }
  };

  let lead = "Checking...";
  let items: Item[] = [];
  if (st?.llm) {
    items = [
      {
        title: "The clock is set",
        done: !!st.clock_valid,
        why: "Reminders and alarms wait until the device gets the time from the network.",
      },
      {
        title: "Admin password",
        done: s.mode === "password",
        why: "Anyone on your network can use this page.",
        action: ["settings", "Set a password"],
      },
      {
        title: "Language model",
        done: !!st.llm.configured,
        why: "Without one the speaker only understands simple commands like timers.",
        action: ["settings", "See how"],
      },
      {
        title: "A speaker is connected",
        done: (st.satellites ?? 0) > 0,
        why: "Start a speaker program on the device or a computer so it can listen.",
      },
      {
        title: "An agent is connected",
        done: ov.endpoints.length > 0,
        why: "Create a webhook so Claude Code or another agent can talk to the speaker.",
        action: ["webhooks", "Connect an agent"],
      },
      { title: "Speaker test", done: s.spoke, why: "Use the box below and listen for it." },
    ];
    const todo = items.filter((i) => !i.done).length;
    lead =
      todo === 0
        ? "Everything is set up and running."
        : todo === 1
          ? "One thing left to set up."
          : todo + " things left to set up.";
  }
  const u = st?.usage;

  return (
    <>
      <h2>Home</h2>
      <p class="lead">{lead}</p>
      <ul class="check">
        {items.map((i) => (
          <li key={i.title} class={i.done ? "done" : "todo"}>
            <div>
              <div>
                <span class="sr">{i.done ? "Done: " : "To do: "}</span>
                {i.title}
              </div>
              {i.done ? null : <div class="why">{i.why}</div>}
              {!i.done && i.action ? (
                <div class="acts">
                  <button class="plain" type="button" onClick={() => go(i.action?.[0] ?? "home")}>
                    {i.action[1]}
                  </button>
                </div>
              ) : null}
            </div>
          </li>
        ))}
      </ul>
      <p class="mut">
        {u
          ? `Today: ${u.requests_today} of ${u.max_requests_per_day} language model requests${u.spend_today > 0 ? `, $${u.spend_today.toFixed(3)} spent` : ""}.`
          : null}
      </p>
      <form class="panel" onSubmit={speak}>
        <h3>Test the speaker</h3>
        <p class="mut">Type a sentence and the speaker will say it out loud.</p>
        <div class="row">
          <input
            id="speaktext"
            type="text"
            placeholder="For example: Hello from the kitchen"
            aria-label="What the speaker should say"
            maxLength={200}
            value={text}
            onInput={(e) => setText(e.currentTarget.value)}
          />
          <button>Say it</button>
        </div>
      </form>
    </>
  );
}
