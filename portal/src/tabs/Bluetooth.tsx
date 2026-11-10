import type { JSX } from "preact";

/** Pairing speakers, headphones and phones. The device layer has no Bluetooth interface yet (spec 0006), so this page
 *  says plainly what it will do and that it cannot yet, instead of showing a list that would be made up. */
export function Bluetooth(): JSX.Element {
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Bluetooth</h2>
        <p>Connect headphones, a phone or another speaker to this device.</p>
      </header>
      <div class="stack">
        <section class="card" id="btcard">
          <div class="head">
            <h3>Paired devices</h3>
            <button class="plain small" type="button" disabled>
              Start pairing
            </button>
          </div>
          <div class="blank">
            <span class="fico" aria-hidden="true">
              <svg class="ico" viewBox="0 0 24 24">
                <path d="m7 7 10 10-5 5V2l5 5L7 17" />
              </svg>
            </span>
            <h3>Bluetooth is not available on this device yet</h3>
            <p class="mut">
              The device software does not have a Bluetooth connection yet. When it does, you will see paired devices
              here, and you can pair a new one or remove an old one.
            </p>
          </div>
        </section>
        <section class="card">
          <h3>What it will do</h3>
          <ul class="rows">
            <li>
              <strong>Play to a Bluetooth speaker or headphones</strong>
              <div class="mut">The assistant's voice and sounds go there instead of the built-in speaker.</div>
            </li>
            <li>
              <strong>Pair a phone</strong>
              <div class="mut">
                Hold the action button, or choose Start pairing here, and pick the speaker on your phone.
              </div>
            </li>
            <li>
              <strong>Forget a device</strong>
              <div class="mut">Remove one you no longer use, so it stops connecting.</div>
            </li>
          </ul>
        </section>
      </div>
    </div>
  );
}
