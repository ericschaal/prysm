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
 * its place; a live problem joins it in the error colour after `lead` (the
 * device's name), which gives way first when the line is short, so the
 * problem is read whole and nothing moves.
 */
export function RowLink({
  label,
  value,
  lead,
  problem = null,
  onClick,
}: {
  label: string;
  value: string;
  /** Kept before a problem: what the row is about, such as the device. */
  lead?: string;
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
            {problem === null ? (
              value
            ) : (
              <>
                {lead ? <span className="row-lead">{lead} · </span> : null}
                <span className="row-problem">{problem}</span>
              </>
            )}
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

function CheckIcon() {
  return (
    <svg
      className="row-check"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M5 12.5 10 17.5 19 7" />
    </svg>
  );
}

/**
 * One option of a choice in the rows pattern: a radio drawn as a tick in
 * the chevron's place. Its label names it; the line beneath (and `facts`
 * beside the label, if given) describe it, so a screen reader reads the
 * name first and the trade after it.
 */
export function RowChoice({
  name,
  label,
  about,
  facts,
  checked,
  disabled = false,
  onChoose,
}: {
  /** The radio group's name: one per choice. */
  name: string;
  label: string;
  about: string;
  /** What the option sets, quietly beside its label: `640 × 360 · 30 fps`. */
  facts?: string;
  checked: boolean;
  disabled?: boolean;
  onChoose: () => void;
}) {
  const id = useId();
  return (
    <li className="row row-choice">
      <label>
        <input
          type="radio"
          name={name}
          checked={checked}
          disabled={disabled}
          aria-labelledby={`${id}-label`}
          aria-describedby={facts ? `${id}-about ${id}-facts` : `${id}-about`}
          onChange={onChoose}
        />
        <span className="row-text">
          {facts ? (
            <span className="row-heading">
              <span className="row-label" id={`${id}-label`}>
                {label}
              </span>
              <span className="row-value" id={`${id}-facts`}>
                {facts}
              </span>
            </span>
          ) : (
            <span className="row-label" id={`${id}-label`}>
              {label}
            </span>
          )}
          <span className="row-value" id={`${id}-about`}>
            {about}
          </span>
        </span>
        <CheckIcon />
      </label>
    </li>
  );
}

/** A row holding one plain on/off switch (a `CheckboxField`). */
export function RowSwitch({ children }: { children: ReactNode }) {
  return <li className="row row-switch">{children}</li>;
}
