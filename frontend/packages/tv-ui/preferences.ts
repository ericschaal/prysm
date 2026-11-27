/** The saved preferences as the app edits them, and Prysm's address. */
import type { Preferences } from '@prysm/webos-platform';

const PORTAL_PORT = 8787;

/** Preferences with every field present (no address: `null`), as the app edits them. */
export type SavedPreferences = {
  autostart: boolean;
  prysm_address: string | null;
};
export function completePreferences(value: Preferences): SavedPreferences {
  return {
    autostart: value.autostart ?? false,
    prysm_address: value.prysm_address ?? null,
  };
}

/** `saved` as the service takes it: no address is omitted. */
export function sentPreferences(saved: SavedPreferences): Preferences {
  const { autostart, prysm_address } = saved;
  return prysm_address === null ? { autostart } : { autostart, prysm_address };
}

/**
 * The portal origin from what a user typed: a host name or IP becomes
 * `http://host:8787`; a full `http(s)://` origin is kept; a trailing `/` is
 * dropped. Returns an error text for anything that is not an origin.
 */
export function prysmAddress(
  input: string,
): { value: string | null } | { error: string } {
  const text = input.trim().replace(/\/+$/, '');
  if (!text) return { value: null };
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(text)
    ? text
    : `http://${text}${/:\d+$/.test(text) && !/^\[[^\]]*\]$/.test(text) ? '' : `:${PORTAL_PORT}`}`;
  if (
    !/^https?:\/\/(\[[0-9a-f:.]+\]|[^\s/?#@[\]:]+)(:\d{1,5})?$/i.test(
      withScheme,
    )
  )
    return {
      error: 'That doesn’t look like an address. Try one like 192.168.1.20.',
    };
  return {
    value: withScheme.replace(/^[a-z]+/i, (scheme) => scheme.toLowerCase()),
  };
}

/**
 * The full worker binds `0.0.0.0`; the TV app reaches it on the loopback
 * address with the advertised port, which stays right when the LAN address
 * changes.
 */
export function localPortal(portal: string | null | undefined): string | null {
  if (!portal) return null;
  const match = /^https?:\/\/(?:\[[^\]]+\]|[^/:]+)(?::(\d+))?/i.exec(portal);
  if (!match) return null;
  return `http://127.0.0.1:${match[1] ?? '80'}`;
}

/** An address as people read and type it: no `http://`, no trailing slash. */
export const shown = (address: string) =>
  address.replace(/^http:\/\//i, '').replace(/\/+$/, '');
