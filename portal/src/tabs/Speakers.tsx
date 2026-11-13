import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api, ApiError } from "../api.ts";
import { fail, usePolled } from "../store.ts";
import { peerLink } from "../forms.ts";
import { span } from "../time.ts";

interface Peer {
  id: string;
  name: string;
  address: string;
  port: number;
  last_seen_secs: number;
}
interface Peers {
  discovery: string;
  self: { id: string; name: string; port: number };
  peers: Peer[];
}

/** The other ClawDot speakers found on this network. Finding them is all that exists so far: nothing is shared yet. */
export function Speakers(): JSX.Element {
  const [data, setData] = useState<Peers | null>(null);
  const [off, setOff] = useState(false);
  usePolled(async () => {
    try {
      setData(await api<Peers>("GET", "/api/hive/peers"));
      setOff(false);
    } catch (x) {
      // Not built in, or not started on this device: a normal state, not a failure.
      if (x instanceof ApiError && x.status === 404) setOff(true);
      else fail(x);
    }
  });
  return (
    <section class="card" id="speakerscard">
      <div class="head">
        <h3>
          Other speakers<span class="count">{data?.peers.length ?? 0}</span>
        </h3>
      </div>
      <p class="mut">
        ClawDot speakers on the same network can find each other. That is the first step toward speakers that share what
        they know; for now they are only listed, and nothing is shared between them.
      </p>
      {off ? (
        <div class="blank" id="speakersoff">
          <h3>Not looking for other speakers</h3>
          <p class="mut">
            Finding other speakers is not turned on for this device. It is switched on where ClawDot is started, with
            the hive discovery option.
          </p>
        </div>
      ) : data ? (
        <>
          <dl class="kv">
            <dt>This speaker</dt>
            <dd id="selfname">{data.self.name}</dd>
            <dt>Its id</dt>
            <dd class="idtext">{data.self.id.slice(0, 16)}</dd>
          </dl>
          {data.peers.length ? (
            <ul class="rows tools" id="peerlist">
              {data.peers.map((p) => (
                <li key={p.id} class="toolrow memrow">
                  <div class="toolmain">
                    <div class="skillname">{p.name}</div>
                    <div class="toolorigin">
                      {p.address}, seen {p.last_seen_secs < 5 ? "just now" : span(p.last_seen_secs) + " ago"}
                    </div>
                  </div>
                  {peerLink(p.address, p.port) ? (
                    <a href={peerLink(p.address, p.port) ?? undefined} rel="noreferrer">
                      Open its page
                    </a>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : (
            <p class="empty" id="nopeers">
              No other speakers found yet. A speaker that is switched on shows up here within a minute.
            </p>
          )}
        </>
      ) : (
        <p class="empty">Looking.</p>
      )}
    </section>
  );
}
