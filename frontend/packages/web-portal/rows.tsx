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

export function ChevronIcon() {
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

/** A choice's mark: a ring, filled and ticked while chosen. */
function CheckIcon() {
  return (
    <svg
      className="row-check"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <circle className="row-ring" cx="12" cy="12" r="10.5" />
      <path className="row-tick" d="M7.5 12.4 10.6 15.5 16.5 9" />
    </svg>
  );
}

/**
 * One option of a choice in the rows pattern: a radio drawn as a ring in
 * the chevron's place, filled and ticked while chosen. Its label names it; the line beneath (and `facts`
 * beside the label, if given) describe it, so a screen reader reads the
 * name first and the trade after it. A `problem` joins that line in the
 * error colour after `about`, as in a `RowLink`.
 */
export function RowChoice({
  name,
  label,
  about,
  problem,
  facts,
  value,
  checked,
  disabled = false,
  onChoose,
}: {
  /** The radio group's name: one per choice. */
  name: string;
  /** The radio's value, such as an adapter kind. */
  value?: string;
  label: string;
  /** What choosing it means; absent: the label says enough. */
  about?: string;
  /** What is wrong with it now, such as `Not plugged in`. */
  problem?: string;
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
          value={value}
          checked={checked}
          disabled={disabled}
          aria-labelledby={`${id}-label`}
          aria-describedby={
            [(about || problem) && `${id}-about`, facts && `${id}-facts`]
              .filter(Boolean)
              .join(' ') || undefined
          }
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
          {problem ? (
            <span className="row-value" id={`${id}-about`} data-problem="true">
              {about ? <span className="row-lead">{about} · </span> : null}
              <span className="row-problem">{problem}</span>
            </span>
          ) : null}
          {about && !problem ? (
            <span className="row-value" id={`${id}-about`}>
              {about}
            </span>
          ) : null}
        </span>
        <CheckIcon />
      </label>
    </li>
  );
}

/**
 * A row that shows or hides what follows it, below the row: a chevron in
 * the link's place points down, and up while open. Opening adds to the end
 * of the page, so nothing above it moves.
 */
export function RowDisclosure({
  heading,
  label,
  about,
  open,
  onToggle,
  children,
}: {
  /** The group's small heading: without one the row reads as an empty cell. */
  heading: string;
  label: string;
  /** The line beneath the label, the same open or closed. */
  about: string;
  open: boolean;
  onToggle: () => void;
  /** Drawn only while open. */
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section className="group" aria-labelledby={`${id}-heading`}>
      <h2 id={`${id}-heading`} className="overline">
        {heading}
      </h2>
      <div className="rows">
        <div className="row">
          <button
            type="button"
            className="row-link row-toggle"
            aria-expanded={open}
            aria-controls={id}
            aria-describedby={`${id}-about`}
            onClick={onToggle}
          >
            <span className="row-text">
              <span className="row-label">{label}</span>
              <span className="row-value" id={`${id}-about`}>
                {about}
              </span>
            </span>
            <ChevronIcon />
          </button>
        </div>
      </div>
      <div id={id} className="disclosure-body">
        {open ? children : null}
      </div>
    </section>
  );
}

/** A row holding one plain on/off switch (a `CheckboxField`). */
export function RowSwitch({ children }: { children: ReactNode }) {
  return <li className="row row-switch">{children}</li>;
}
