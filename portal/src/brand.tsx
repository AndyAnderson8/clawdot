// The mark and the name, shared by the app's rail and the sign-in pages.
import type { JSX } from "preact";

export const Logo = (): JSX.Element => (
  <svg class="logo" aria-hidden="true">
    <use href="#mk" />
  </svg>
);

/** The name, with the light ring of the speaker standing in for the o. Read out as plain "ClawDot". */
export function Wordmark(): JSX.Element {
  return (
    <span class="wordmark" aria-hidden="true">
      ClawD
      <span class="ring" />t
    </span>
  );
}
