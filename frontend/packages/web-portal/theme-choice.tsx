import { useId, useState } from 'react';
import { RowChoice } from './rows';
import {
  applyTheme,
  saveTheme,
  currentTheme,
  themeChoices,
  type Theme,
} from './theme';

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
          <RowChoice
            key={choice.value}
            name={nameId}
            label={choice.label}
            about={choice.about}
            checked={theme === choice.value}
            onChoose={() => choose(choice.value)}
          />
        ))}
      </ul>
    </section>
  );
}
