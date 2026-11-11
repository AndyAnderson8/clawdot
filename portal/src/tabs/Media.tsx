import type { JSX } from "preact";

/** What a media source is for, shown on both the Media page and in its settings. */
const KINDS: { id: string; name: string; what: string }[] = [
  { id: "music", name: "Music", what: "Songs, albums and playlists from a music service." },
  { id: "podcasts", name: "Podcasts", what: "Shows and episodes, with the place you stopped remembered." },
  { id: "radio", name: "Radio", what: "Internet radio stations by name." },
];

/** Playing music, podcasts and radio, and the Bluetooth connection. Nothing here can play yet, and the page says so
 *  instead of showing controls that would do nothing (the device layer has no Bluetooth interface yet, spec 0006). */
export function Media(): JSX.Element {
  return (
    <div class="page">
      <header class="pagehead">
        <h2>Media</h2>
        <p>Music, podcasts and radio, and how this device connects to speakers and phones.</p>
      </header>
      <div class="stack">
        <section class="card" id="mediacard">
          <h3>Music, podcasts and radio</h3>
          <div class="blank">
            <span class="fico" aria-hidden="true">
              <svg class="ico" viewBox="0 0 24 24">
                <path d="M9 18V5l12-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0Zm12-2a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" />
              </svg>
            </span>
            <h3>Nothing to play yet</h3>
            <p class="mut">
              Choose where your music, podcasts and radio come from, and you will be able to start them here or by
              voice.
            </p>
            <a class="btn" id="mediasettings" href="#/settings/media">
              Set up media providers
            </a>
          </div>
        </section>
        <section class="card" id="btcard">
          <h3>Bluetooth</h3>
          <div class="srow">
            <div class="slabel">
              <label for="btswitch">Bluetooth</label>
              <p class="mut">
                Not available on this device yet. The device software has no Bluetooth connection, so this cannot be
                turned on from here. Hold the action button to start pairing.
              </p>
            </div>
            <div class="sctl">
              <input
                id="btswitch"
                class="switch"
                type="checkbox"
                disabled
                checked={false}
                aria-label="Bluetooth is off"
              />
            </div>
          </div>
          <ul class="rows">
            <li>
              <strong>Play to a Bluetooth speaker or headphones</strong>
              <div class="mut">The assistant's voice and sounds go there instead of the built-in speaker.</div>
            </li>
            <li>
              <strong>Pair a phone</strong>
              <div class="mut">Choose the speaker on your phone to play your own music through it.</div>
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

/** Settings, Media: where the providers will be chosen. The choices are shown but cannot be used yet. */
export function MediaSettings(): JSX.Element {
  return (
    <section class="card" id="mediaproviders">
      <h3>Media providers</h3>
      <p class="mut">
        A provider is the service your music, podcasts or radio come from. Choosing one is not possible yet: these
        choices open up when ClawDot can play media.
      </p>
      {KINDS.map((k) => (
        <div class="srow" key={k.id}>
          <div class="slabel">
            <label for={"media-" + k.id}>{k.name}</label>
            <p class="mut">{k.what}</p>
          </div>
          <div class="sctl">
            <select id={"media-" + k.id} disabled>
              <option>Not set up</option>
            </select>
          </div>
        </div>
      ))}
    </section>
  );
}
