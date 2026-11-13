import type { JSX } from "preact";
import { useApp } from "../store.ts";
import type { HardwareInfo } from "../types.ts";

/** What to call the device: the configured label, else the model it gives itself, else a plain "Unknown device". */
export function deviceName(hw: HardwareInfo): string {
  return hw.label || hw.model || "Unknown device";
}

/** Whether a part's answer came from the configuration file instead of from looking. */
function badge(hw: HardwareInfo, k: keyof HardwareInfo["parts"]): [string, string] {
  const configured = hw.configured_parts.includes(k);
  if (hw.parts[k]) return [configured ? "Set in configuration" : "Found", "good"];
  return [configured ? "Turned off in configuration" : "Not found", ""];
}

const PARTS: [keyof HardwareInfo["parts"], string, string][] = [
  ["leds", "Light ring", "Lights the speaker can show what it is doing with."],
  ["bluetooth", "Bluetooth", "Pairs speakers, headphones and phones."],
  ["wifi", "Wi-Fi", "A wireless network adapter."],
  ["screen", "Screen", "A display to show what is happening."],
  ["usb_serial", "USB serial adapter", "What a Zigbee or Z-Wave adapter plugs in as. ClawDot cannot tell which it is."],
];

/** What this device is and which parts were found on it, so settings it cannot use can be left out. */
export function HardwareCard(): JSX.Element {
  const hw = useApp().status?.hardware;
  return (
    <section class="card" id="hardwarecard">
      <h3>This device</h3>
      <p class="mut">What ClawDot found by looking at the device it is running on.</p>
      {!hw ? (
        <p class="empty">Loading.</p>
      ) : (
        <>
          <dl class="kv">
            <dt>Device</dt>
            <dd id="hwboard">{deviceName(hw)}</dd>
            {hw.model && hw.model !== deviceName(hw) ? (
              <>
                <dt>Model</dt>
                <dd>{hw.model}</dd>
              </>
            ) : null}
            {hw.cpu ? (
              <>
                <dt>Processor</dt>
                <dd>
                  {hw.cpu}
                  {hw.cores ? ", " + hw.cores + (hw.cores === 1 ? " core" : " cores") : ""}
                </dd>
              </>
            ) : null}
            {hw.ram_mb ? (
              <>
                <dt>Memory</dt>
                <dd>{hw.ram_mb} MB</dd>
              </>
            ) : null}
            <dt>Software</dt>
            <dd>
              {hw.os}, {hw.arch}
            </dd>
          </dl>
          <ul class="rows hwparts" id="hwparts">
            {PARTS.map(([k, name, what]) => {
              const [text, tone] = badge(hw, k);
              return (
                <li key={k} class="hwpart">
                  <span class={"badge " + tone}>{text}</span>
                  <div>
                    <strong>{name}</strong>
                    <div class="mut">{what}</div>
                  </div>
                </li>
              );
            })}
          </ul>
          {hw.board === "unknown" ? (
            <p class="mut" id="hwunknown">
              No device preset is set up, so only what the operating system shows was used. A part it cannot show, such
              as a light ring another program drives, can be declared in the device section of the configuration file.
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}
