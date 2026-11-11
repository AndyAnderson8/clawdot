import type { JSX } from "preact";

/** A section that is planned but not built: it says what it will do, and nothing else. */
export function Planned(props: { title: string; text: string }): JSX.Element {
  return (
    <section class="card" id="planned">
      <h3>{props.title}</h3>
      <div class="blank">
        <span class="fico" aria-hidden="true">
          <svg class="ico" viewBox="0 0 24 24">
            <path d="M12 6v6l4 2M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z" />
          </svg>
        </span>
        <h3>Not available yet</h3>
        <p class="mut">{props.text}</p>
      </div>
    </section>
  );
}
