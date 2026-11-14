import { useId, useState } from 'react';
import {
  applyTheme,
  saveTheme,
  currentTheme,
  themeChoices,
  type Theme,
} from './theme';

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
 * System, Light or Dark: one row each, in the rows pattern, the chosen one
 * ticked. Applied at once and kept in this browser.
 */
export function ThemeChoice() {
  const [theme, setTheme] = useState(currentTheme);
  const nameId = useId();
  const headingId = useId();
  function choose(next: Theme) {
    setTheme(next);
    applyTheme(next);
    saveTheme(next);
  }
  return (
    <section className="group" aria-labelledby={headingId}>
      <h2 id={headingId} className="overline">
        Theme
      </h2>
      <ul className="rows" role="radiogroup" aria-labelledby={headingId}>
        {themeChoices.map((choice) => (
          <li key={choice.value} className="row row-choice">
            <label>
              <input
                type="radio"
                name={nameId}
                value={choice.value}
                checked={theme === choice.value}
                onChange={() => choose(choice.value)}
              />
              <span className="row-text">
                <span className="row-label">{choice.label}</span>
                <span className="row-value">{choice.about}</span>
              </span>
              <CheckIcon />
            </label>
          </li>
        ))}
      </ul>
    </section>
  );
}
