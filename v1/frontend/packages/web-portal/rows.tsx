import { useId, type ReactNode } from 'react';

/**
 * The one list pattern of Settings and its pages: a small heading over a
 * group of rows, each a label with its current value on the line beneath.
 */
export function Group({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  const headingId = useId();
  return (
    <section className="group" aria-labelledby={headingId}>
      <h2 id={headingId} className="overline">
        {title}
      </h2>
      <ul className="rows">{children}</ul>
    </section>
  );
}

function ChevronIcon() {
  return (
    <svg
      className="chevron"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M9 5.5 15.5 12 9 18.5" />
    </svg>
  );
}

/**
 * A row that opens a page: the whole row is the link. Its value line keeps
 * its place; a live problem takes it, in the error colour, so nothing moves.
 */
export function RowLink({
  label,
  value,
  problem = null,
  onClick,
}: {
  label: string;
  value: string;
  /** What is wrong with this part now; `null` while it works. */
  problem?: string | null;
  onClick: () => void;
}) {
  const valueId = useId();
  return (
    <li className="row">
      <button
        type="button"
        className="row-link"
        aria-describedby={valueId}
        onClick={onClick}
      >
        <span className="row-text">
          <span className="row-label">{label}</span>
          <span
            id={valueId}
            className="row-value"
            data-problem={problem !== null}
            aria-live="polite"
          >
            {problem ?? value}
          </span>
        </span>
        <ChevronIcon />
      </button>
    </li>
  );
}

/** A row that only says something: a label and its value. */
export function RowFact({ label, value }: { label: string; value: string }) {
  return (
    <li className="row row-fact">
      <span className="row-text">
        <span className="row-label">{label}</span>
        <span className="row-value">{value}</span>
      </span>
    </li>
  );
}

/** A row holding one plain on/off switch (a `CheckboxField`). */
export function RowSwitch({ children }: { children: ReactNode }) {
  return <li className="row row-switch">{children}</li>;
}
