import type { JSX } from "preact";

/** What the ring shows for each thing the assistant does. The colours here are the planned defaults, drawn so the page
 *  shows what the ring will look like; they are not read from the device (it has no interface for them yet). */
const STATES: { id: string; name: string; what: string; color: string; lit: number }[] = [
  {
    id: "idle",
    name: "Waiting",
    what: "Listening for its name. Dim, so it does not light the room.",
    color: "#2f6f7a",
    lit: 1,
  },
  { id: "listening", name: "Listening", what: "It heard its name and is hearing you.", color: "#3ccfb4", lit: 12 },
  { id: "thinking", name: "Thinking", what: "Working on your request.", color: "#e8b341", lit: 4 },
  { id: "speaking", name: "Speaking", what: "Answering out loud.", color: "#5aa9e6", lit: 12 },
  { id: "muted", name: "Microphone off", what: "It cannot hear anything.", color: "#d6453d", lit: 12 },
];
const LEDS = 12;

/** A ring of twelve lights, `lit` of them on. */
function Ring(props: { color: string; lit: number; label: string }): JSX.Element {
  const dots = [];
  for (let i = 0; i < LEDS; i++) {
    const a = (i / LEDS) * Math.PI * 2 - Math.PI / 2;
    const on = i < props.lit;
    dots.push(
      <circle
        key={i}
        cx={20 + 15 * Math.cos(a)}
        cy={20 + 15 * Math.sin(a)}
        r={2.4}
        fill={on ? props.color : "currentColor"}
        opacity={on ? 1 : 0.18}
      />,
    );
  }
  return (
    <svg class="ledring" viewBox="0 0 40 40" role="img" aria-label={props.label}>
      {dots}
    </svg>
  );
}

/** Settings, Lights: the ring on top of the speaker. Shown as a preview only, because ClawDot cannot change the
 *  colours or brightness yet. */
export function Lights(): JSX.Element {
  return (
    <>
      <section class="card" id="lightscard">
        <h3>Light ring</h3>
        <p class="mut">
          The ring on top of the speaker shows what the assistant is doing. Choosing its colours and brightness is not
          possible yet. This is how it will look.
        </p>
        <div class="srow">
          <div class="slabel">
            <label for="ledbright">Brightness</label>
            <p class="mut">How bright the ring is.</p>
          </div>
          <div class="sctl">
            <input id="ledbright" type="range" min={0} max={100} value={60} disabled />
          </div>
        </div>
        <ul class="rows ledrows">
          {STATES.map((s) => (
            <li key={s.id} class="ledrow">
              <Ring color={s.color} lit={s.lit} label={s.name + " preview"} />
              <div>
                <strong>{s.name}</strong>
                <div class="mut">{s.what}</div>
              </div>
              <div class="ledcolor">
                <label class="sr" for={"led-" + s.id}>
                  Colour for {s.name}
                </label>
                <input id={"led-" + s.id} type="color" value={s.color} disabled />
              </div>
            </li>
          ))}
        </ul>
      </section>
    </>
  );
}
