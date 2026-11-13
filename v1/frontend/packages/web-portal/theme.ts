/** The portal's colour theme: the system's, or a fixed one. */
export type Theme = 'system' | 'light' | 'dark';

/** Each theme in words, for its row and for the Settings row's value. */
export const themeChoices: readonly {
  value: Theme;
  label: string;
  about: string;
}[] = [
  {
    value: 'system',
    label: 'System',
    about: 'Light or dark, as this device is.',
  },
  { value: 'light', label: 'Light', about: 'Always light.' },
  { value: 'dark', label: 'Dark', about: 'Always dark.' },
];

/** A per-browser preference: kept in local storage, never sent to Prysm. */
export const themeStorageKey = 'prysm.theme';

/** The stored choice; `system` without one or without storage. */
export function storedTheme(): Theme {
  try {
    const value = window.localStorage.getItem(themeStorageKey);
    return value === 'light' || value === 'dark' ? value : 'system';
  } catch {
    return 'system';
  }
}

/** The theme in use on this page: the one applied, else the system's. */
export function currentTheme(): Theme {
  const value = document.documentElement.getAttribute('data-theme');
  return value === 'light' || value === 'dark' ? value : 'system';
}

/**
 * Set `data-theme` on the root element, which the tokens key off; without
 * it the system's `prefers-color-scheme` decides.
 */
export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', theme);
}

/** Remember `theme` in this browser; without storage it lasts the page. */
export function saveTheme(theme: Theme) {
  try {
    if (theme === 'system') window.localStorage.removeItem(themeStorageKey);
    else window.localStorage.setItem(themeStorageKey, theme);
  } catch {
    // Storage is unavailable (private window, blocked): the choice is not kept.
  }
}

// Applied when the bundle loads, before React renders: the portal's CSP
// allows no inline script in the page head.
if (typeof document !== 'undefined') applyTheme(storedTheme());
