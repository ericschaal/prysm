import { use, useEffect, useRef, useState } from 'react';
import {
  type Candidate,
  type DiscoveryClient,
} from '@prysm/application-client';
import { DiscoveryContext } from './discovery-context';
import { useFieldScopeContext } from './form';

type Search =
  | { state: 'idle' }
  | { state: 'looking' }
  | { state: 'found'; found: Candidate[] }
  | { state: 'failed' };

/** What a candidate's line under its name says: `192.168.1.40, 120 LEDs`. */
const detailOf = ({ detail, led_count }: Candidate) =>
  [detail, led_count === undefined ? undefined : `${led_count} LEDs`]
    .filter(Boolean)
    .join(', ');

/**
 * A button that asks the adapter for `kind` what it finds (devices present,
 * lights on the network) and lists them; picking one fills the field `path`
 * with the candidate's `fields[field]` and hands the editor the candidate.
 * The typed field stays the fallback.
 * Nothing is shown without a discovery client (the portal's `discovery`).
 */
export function FindDevices({
  section,
  kind,
  path,
  field,
  label,
  looking,
  nothing,
  fallback,
  onPick,
}: {
  section: 'capture' | 'output';
  kind: string;
  /** The field it fills, for its errors, such as `capture.path`. */
  path: string;
  /** The key in a candidate's `fields`, such as `path`. */
  field: string;
  label: string;
  looking: string;
  /** Shown when nothing is found, such as `No devices found.` */
  nothing: string;
  /** What to do instead, such as `Enter the path below.` */
  fallback: string;
  onPick: (value: string, candidate: Candidate) => void;
}) {
  const discovery = use(DiscoveryContext);
  const scope = useFieldScopeContext();
  const [search, setSearch] = useState<Search>({ state: 'idle' });
  const [picked, setPicked] = useState('');
  /** Ends a lookup the editor no longer shows. */
  const abortRef = useRef<AbortController | null>(null);
  useEffect(() => () => abortRef.current?.abort(), []);
  if (!discovery) return null;

  async function find(client: DiscoveryClient) {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    setSearch({ state: 'looking' });
    setPicked('');
    try {
      const found = await client.candidates(section, kind, controller.signal);
      if (controller.signal.aborted) return;
      setSearch({
        state: 'found',
        found: found.filter(
          (candidate) => typeof candidate.fields[field] === 'string',
        ),
      });
    } catch {
      if (!controller.signal.aborted) setSearch({ state: 'failed' });
    }
  }

  let result = null;
  if (search.state === 'failed')
    result = (
      <p className="notice warning" role="status">
        Couldn’t search just now. {fallback}
      </p>
    );
  else if (search.state === 'found' && !search.found.length)
    result = (
      <p className="notice" role="status">
        {nothing} {fallback}
      </p>
    );
  else if (search.state === 'found')
    result = (
      <ul className="found" aria-label="Found">
        {search.found.map((candidate) => {
          const value = candidate.fields[field] as string;
          const detail = detailOf(candidate);
          return (
            <li key={value}>
              <button
                type="button"
                disabled={scope.disabled}
                aria-pressed={picked === value}
                onClick={() => {
                  scope.edited(path);
                  setPicked(value);
                  onPick(value, candidate);
                }}
              >
                <strong>{candidate.label}</strong>
                {detail ? <span className="found-detail">{detail}</span> : null}
              </button>
            </li>
          );
        })}
      </ul>
    );
  return (
    <div className="find">
      <button
        type="button"
        disabled={scope.disabled || search.state === 'looking'}
        onClick={() => void find(discovery)}
      >
        {search.state === 'looking' ? looking : label}
      </button>
      {result}
    </div>
  );
}
