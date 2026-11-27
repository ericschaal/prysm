// Keep the storage key and theme values in sync with theme.ts.
try {
  const theme = window.localStorage.getItem('prysm.theme');
  if (theme === 'light' || theme === 'dark')
    document.documentElement.setAttribute('data-theme', theme);
} catch {
  // Leave the system theme in effect when storage is unavailable.
}
