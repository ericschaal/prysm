import { useSyncExternalStore } from 'react';

/**
 * Developer options: the video sources and lights meant for development and
 * tests (a video file, no lights) are offered only with them on. A setup
 * already of such a kind always shows and stays editable. A per-browser
 * preference, kept in local storage and never sent to Prysm.
 */
export const developerStorageKey = 'prysm.developer';

/** Told when the preference changes in this page (`storage` covers others). */
const changed = 'prysm-developer';

/** Fallback for the current page when storage is unavailable. */
let unstored = false;

function storedDeveloper() {
  try {
    return window.localStorage.getItem(developerStorageKey) === 'on';
  } catch {
    return unstored;
  }
}

/** Turn developer options on or off in this browser. */
export function saveDeveloper(on: boolean) {
  unstored = on;
  try {
    if (on) window.localStorage.setItem(developerStorageKey, 'on');
    else window.localStorage.removeItem(developerStorageKey);
  } catch {
    // Keep the in-memory choice if storage is unavailable.
  }
  window.dispatchEvent(new window.Event(changed));
}

function subscribe(notify: () => void) {
  window.addEventListener(changed, notify);
  window.addEventListener('storage', notify);
  return () => {
    window.removeEventListener(changed, notify);
    window.removeEventListener('storage', notify);
  };
}

/** Whether developer options are on in this browser, kept up to date. */
export function useDeveloper() {
  return useSyncExternalStore(subscribe, storedDeveloper, () => false);
}
