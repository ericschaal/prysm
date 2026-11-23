import { useEffect, useState } from 'react';
import type {
  Candidate,
  DiscoveryClient,
  FailureSection,
} from '@prysm/application-client';
import type { AdapterFact, AdapterSection } from './adapters';

/** A configured section for Home: what it is, and a few facts about it. */
export interface SetupPart {
  section: FailureSection;
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
 * Discovered device names, kept in this browser: per section and kind, the
 * last few devices seen, each with the candidate fields that identify it (a
 * WLED address, a capture device path). Shown at once on the next visit,
 * and for a saved device that is not found now; never sent to Prysm.
 */
export const deviceNamesKey = 'prysm.deviceNames';

/** How many devices are remembered per section and kind. */
const keptPerKind = 8;

interface KnownName {
  fields: Record<string, unknown>;
  name: string;
}

function readNames(): Record<string, KnownName[]> {
  try {
    const stored = window.localStorage.getItem(deviceNamesKey);
    const names: unknown = stored ? JSON.parse(stored) : {};
    return names && typeof names === 'object'
      ? (names as Record<string, KnownName[]>)
      : {};
  } catch {
    return {};
  }
}

function writeNames(names: Record<string, KnownName[]>) {
  try {
    window.localStorage.setItem(deviceNamesKey, JSON.stringify(names));
  } catch {
    // Storage unavailable: names are found again next time.
  }
}

const slotOf = (section: string, kind: string) => `${section}:${kind}`;

/** The name kept for the device `saved` selects, if one was ever seen. */
export function keptName(section: string, saved: AdapterSection) {
  const kept = readNames()[slotOf(section, saved.kind)];
  if (!Array.isArray(kept)) return null;
  return (
    kept.find(
      (known) =>
        known?.fields &&
        typeof known.name === 'string' &&
        picks(known.fields, saved),
    )?.name ?? null
  );
}

/** Keep the names of devices just found, newest first. */
export function keepNames(
  section: string,
  kind: string,
  found: readonly Pick<Candidate, 'label' | 'fields'>[],
) {
  if (!found.length) return;
  const names = readNames();
  const slot = slotOf(section, kind);
  const same = (a: KnownName['fields'], b: KnownName['fields']) =>
    JSON.stringify(a) === JSON.stringify(b);
  const before = Array.isArray(names[slot]) ? names[slot] : [];
  names[slot] = [
    ...found.map(({ label, fields }) => ({ fields, name: label })),
    ...before.filter(
      (known) => !found.some(({ fields }) => same(fields, known?.fields)),
    ),
  ].slice(0, keptPerKind);
  writeNames(names);
}

/**
 * The device's own name, such as the capture card's: the one kept from the
 * last visit at once, refreshed from discovery in the background (which
 * keeps every name it finds); the kind's name only when none is known.
 */
export function useDeviceName(
  discovery: DiscoveryClient | null,
  part: SetupPart,
) {
  const key = JSON.stringify(part.saved);
  const { section } = part;
  const [found, setFound] = useState<{ key: string; name: string | null }>(
    () => ({ key, name: keptName(section, part.saved) }),
  );
  if (found.key !== key) setFound({ key, name: keptName(section, part.saved) });
  useEffect(() => {
    if (!discovery) return;
    // By value: a new object with the same values is the same device.
    const saved = JSON.parse(key) as AdapterSection;
    const abort = new AbortController();
    discovery
      .candidates(section, saved.kind, abort.signal)
      .then((candidates) => {
        if (abort.signal.aborted) return;
        keepNames(section, saved.kind, candidates);
        const match = candidates.find((candidate) =>
          picks(candidate.fields, saved),
        );
        if (match) setFound({ key, name: match.label });
      })
      .catch(() => {
        // Not found or not available: the kept (or kind's) name stays.
      });
    return () => abort.abort();
  }, [discovery, section, key]);
  return (found.key === key ? found.name : null) ?? part.label;
}
