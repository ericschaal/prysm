import { useEffect, useState } from 'react';
import type { Candidate } from '@prysm/application-client';
import { useHost } from './portal-context';
import type { AdapterSection, SetupPart } from './adapters';

/** Whether every field the candidate fills has the saved value. */
function picks(fields: unknown, saved: Record<string, unknown>) {
  const entries: [string, unknown][] = Object.entries(fields ?? {});
  return (
    entries.length > 0 && entries.every(([key, value]) => saved[key] === value)
  );
}

/** Browser storage key for recently discovered device names, grouped by section and kind. */
export const deviceNamesKey = 'prysm.deviceNames';

/** How many devices are remembered per section and kind. */
const keptPerKind = 8;

function cacheObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object';
}

function parseFields(json: string) {
  const value: unknown = JSON.parse(json);
  return cacheObject(value) ? value : {};
}

function readNames(section: string, kind: string) {
  const slot = `${section}:${kind}`;
  let names: Record<string, unknown> = {};
  try {
    names = parseFields(window.localStorage.getItem(deviceNamesKey) ?? '{}');
  } catch {
    // Discovery also works when browser storage is unavailable.
  }
  const value = names[slot];
  const kept: unknown[] = Array.isArray(value) ? value : [];
  function save(entries: unknown[]) {
    names[slot] = entries;
    try {
      window.localStorage.setItem(deviceNamesKey, JSON.stringify(names));
    } catch {
      // Discovery remains available when the cache cannot be saved.
    }
  }
  return { kept, save };
}

/** The name kept for the device `saved` selects, if one was ever seen. */
export function keptName(section: string, saved: AdapterSection) {
  for (const entry of readNames(section, saved.kind).kept) {
    if (!cacheObject(entry)) continue;
    const { name, fields } = entry;
    if (typeof name === 'string' && picks(fields, saved)) return name;
  }
  return null;
}

/** Keep the names of devices just found, newest first. */
export function keepNames(
  section: string,
  kind: string,
  found: readonly Pick<Candidate, 'label' | 'fields'>[],
) {
  if (!found.length) return;
  const { kept, save } = readNames(section, kind);
  const fresh = found.map(({ label, fields }) => ({ fields, name: label }));
  const older = kept.filter((entry) => {
    const fields = cacheObject(entry) ? entry.fields : undefined;
    return !fresh.some(
      (device) => JSON.stringify(device.fields) === JSON.stringify(fields),
    );
  });
  save([...fresh, ...older].slice(0, keptPerKind));
}

/** Show the cached name or adapter label while discovery looks up the selected device. */
export function useDeviceName(part: SetupPart) {
  const { discovery } = useHost();
  const key = JSON.stringify(part.saved);
  const { section } = part;
  const { kind } = part.saved;
  const [found, setFound] = useState<{ key: string; name: string | null }>(
    () => ({ key, name: keptName(section, part.saved) }),
  );
  if (found.key !== key) setFound({ key, name: keptName(section, part.saved) });
  useEffect(() => {
    if (!discovery) return;
    const saved = parseFields(key);
    const abort = new AbortController();
    discovery
      .candidates(section, kind, abort.signal)
      .then((candidates) => {
        if (abort.signal.aborted) return;
        keepNames(section, kind, candidates);
        const match = candidates.find((candidate) =>
          picks(candidate.fields, saved),
        );
        if (match) setFound({ key, name: match.label });
      })
      .catch(() => {
        // Keep the cached name or adapter label when discovery fails.
      });
    return () => abort.abort();
  }, [discovery, section, kind, key]);
  return (found.key === key ? found.name : null) ?? part.label;
}
