import { useId, useState } from 'react';
import { applyTheme, saveTheme, storedTheme, type Theme } from './theme';

const choices: readonly { value: Theme; label: string }[] = [
  { value: 'system', label: 'System' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
];

/** System, Light or Dark, as a segmented control. */
export function ThemeChoice() {
  const [theme, setTheme] = useState(storedTheme);
  const nameId = useId();
  function choose(next: Theme) {
    setTheme(next);
    applyTheme(next);
    saveTheme(next);
  }
  return (
    <fieldset className="segmented theme-choice">
      <legend>Theme</legend>
      <div className="segments">
        {choices.map((choice) => (
          <label key={choice.value}>
            <input
              type="radio"
              name={nameId}
              value={choice.value}
              checked={theme === choice.value}
              onChange={() => choose(choice.value)}
            />
            <span>{choice.label}</span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}
