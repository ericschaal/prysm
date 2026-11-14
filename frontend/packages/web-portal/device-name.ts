import { useEffect, useState } from 'react';
import type { DiscoveryClient } from '@prysm/application-client';
import type { AdapterFact, AdapterSection } from './adapters';

/** A configured section for Home: what it is, and a few facts about it. */
export interface SetupPart {
  section: 'capture' | 'output';
  saved: AdapterSection;
  /** The kind's name, used until (or unless) the device's own is found. */
  label: string;
  facts: AdapterFact[];
}

/** Whether every field the candidate fills has the saved value. */
function picks(fields: Record<string, unknown>, saved: AdapterSection) {
  const values = saved as unknown as Record<string, unknown>;
  const keys = Object.keys(fields);
  return keys.length > 0 && keys.every((key) => fields[key] === values[key]);
}

/**
 * Discovered device names, kept in this browser: one per section and kind,
 * with the candidate fields that identify the device (a WLED address, a
 * capture device path). Shown at once on the next visit; never sent to Prysm.
 */
export const deviceNamesKey = 'prysm.deviceNames';

interface KnownName {
  fields: Record<string, unknown>;
  name: string;
}

function readNames(): Record<string, KnownName> {
  try {
    const stored = window.localStorage.getItem(deviceNamesKey);
    const names: unknown = stored ? JSON.parse(stored) : {};
    return names && typeof names === 'object'
      ? (names as Record<string, KnownName>)
      : {};
  } catch {
    return {};
  }
}

function writeNames(names: Record<string, KnownName>) {
  try {
    window.localStorage.setItem(deviceNamesKey, JSON.stringify(names));
  } catch {
    // Storage unavailable: names are found again next time.
  }
}

/** The kept name for `saved`, dropping one that belongs to another device. */
function knownName(slot: string, saved: AdapterSection) {
  const names = readNames();
  const known = names[slot];
  if (!known) return null;
  if (
    known.fields &&
    typeof known.name === 'string' &&
    picks(known.fields, saved)
  )
    return known.name;
  delete names[slot];
  writeNames(names);
  return null;
}

/**
 * The device's own name, such as the capture card's: the one kept from the
 * last visit at once, refreshed from discovery in the background (and kept
 * again when it changed); the kind's name only when none is known.
 */
export function useDeviceName(
  discovery: DiscoveryClient | null,
  part: SetupPart,
) {
  const key = JSON.stringify(part.saved);
  const { section } = part;
  const slot = `${section}:${part.saved.kind}`;
  const [found, setFound] = useState<{ key: string; name: string | null }>(
    () => ({ key, name: knownName(slot, part.saved) }),
  );
  if (found.key !== key) setFound({ key, name: knownName(slot, part.saved) });
  useEffect(() => {
    if (!discovery) return;
    // By value: a new object with the same values is the same device.
    const saved = JSON.parse(key) as AdapterSection;
    const abort = new AbortController();
    discovery
      .candidates(section, saved.kind, abort.signal)
      .then((candidates) => {
        const match = candidates.find((candidate) =>
          picks(candidate.fields, saved),
        );
        if (!match || abort.signal.aborted) return;
        setFound({ key, name: match.label });
        writeNames({
          ...readNames(),
          [`${section}:${saved.kind}`]: {
            fields: match.fields,
            name: match.label,
          },
        });
      })
      .catch(() => {
        // Not found or not available: the kept (or kind's) name stays.
      });
    return () => abort.abort();
  }, [discovery, section, key]);
  return (found.key === key ? found.name : null) ?? part.label;
}
