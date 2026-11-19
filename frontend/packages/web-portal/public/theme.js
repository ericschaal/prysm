// Runs before the page is drawn, so a theme chosen in this browser never
// flashes the other one first. theme.ts keeps the choice under this key and
// applies it the same way.
try {
  const theme = window.localStorage.getItem('prysm.theme');
  if (theme === 'light' || theme === 'dark')
    document.documentElement.setAttribute('data-theme', theme);
} catch {
  // No storage (private window, blocked): the system's theme.
}
