import { use, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import {
  type Candidate,
  type DiscoveryClient,
} from '@prysm/application-client';
import { keepNames, keptName } from './device-name';
import { DiscoveryContext } from './discovery-context';
import { Advanced } from './fields';
import { shownError, useFieldScopeContext } from './form';
import { RowChoice } from './rows';

type Search =
  | { state: 'idle' }
  /** `again`: asked before, showing what was found then. */
  | { state: 'looking'; found: Candidate[]; again: boolean }
  | { state: 'found'; found: Candidate[] }
  | { state: 'failed' };

/** What a candidate's line under its name says: `192.168.1.40, 120 LEDs`. */
const detailOf = ({ detail, led_count }: Candidate) =>
  [detail, led_count === undefined ? undefined : `${led_count} LEDs`]
    .filter(Boolean)
    .join(', ');

/**
 * Ask the adapter for `kind` what it finds, keeping the candidates that
 * fill `field`, and keep their names in this browser. `auto` asks once at
 * once; `look` asks again, showing the last ones meanwhile. A lookup the
 * editor no longer shows is ended.
 */
function useCandidates(
  discovery: DiscoveryClient | null,
  section: 'capture' | 'output',
  kind: string,
  field: string,
  auto: boolean,
) {
  const [search, setSearch] = useState<Search>(
    auto && discovery
      ? { state: 'looking', found: [], again: false }
      : { state: 'idle' },
  );
  const abortRef = useRef<AbortController | null>(null);
  async function ask(client: DiscoveryClient) {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    try {
      const found = await client.candidates(section, kind, controller.signal);
      if (controller.signal.aborted) return;
      const usable = found.filter(
        (candidate) => typeof candidate.fields[field] === 'string',
      );
      keepNames(section, kind, usable);
      setSearch({ state: 'found', found: usable });
    } catch {
      if (!controller.signal.aborted) setSearch({ state: 'failed' });
    }
  }
  const askRef = useRef(ask);
  useEffect(() => {
    if (auto && discovery) void askRef.current(discovery);
    return () => abortRef.current?.abort();
  }, [auto, discovery]);
  return {
    search,
    looking: search.state === 'looking',
    /** Ask again; ignored while asking, so the control keeps its focus. */
    look: (client: DiscoveryClient) => {
      if (search.state === 'looking') return;
      setSearch({
        state: 'looking',
        found: search.state === 'found' ? search.found : [],
        again: true,
      });
      void ask(client);
    },
  };
}

/**
 * A button that asks the adapter for `kind` what it finds (lights on the
 * network) and lists them; picking one fills the field `path` with the
 * candidate's `fields[field]` and hands the editor the candidate. A button
 * rather than a lookup on opening: WLED discovery takes over a second and
 * asks the network. The typed field stays the fallback.
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
  const {
    search,
    looking: busy,
    look,
  } = useCandidates(discovery, section, kind, field, false);
  const [picked, setPicked] = useState('');
  if (!discovery) return null;

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
        disabled={scope.disabled}
        aria-disabled={busy || undefined}
        aria-busy={busy}
        onClick={() => {
          if (busy) return;
          setPicked('');
          look(discovery);
        }}
      >
        {busy ? looking : label}
      </button>
      {result}
    </div>
  );
}

/**
 * The devices of `kind` present, looked for as the editor opens, as rows
 * to tick, listed once the first lookup has answered and kept while looking
 * again; picking one fills `path` (its field at `field`). The saved
 * device keeps a ticked row of its own while it is not found, by the name
 * this browser last saw for it (else its value), with `missing` beneath in
 * the problem colour. "Look again" sits at the end of the heading line.
 * Under the rows, only while there is something to say, one line says how
 * the lookup went (looking, nothing found, failed), or
 * `problem` (what is wrong with the picked device), or the problem of
 * `path`, such as no device chosen: while there are rows they own `path`,
 * so a save goes to them. The field to type it (`byHand`) is under
 * Advanced, after what follows the device (`then`), while there are
 * devices to pick; with nothing found, or no discovery client, it is shown
 * as it is, before `then`, and owns `path` itself.
 */
export function DeviceRows({
  kind,
  path,
  field,
  legend,
  value,
  missing,
  problem,
  byHand,
  then,
  onPick,
}: {
  kind: string;
  path: string;
  field: string;
  legend: string;
  /** The picked value, such as the device path; `''`: none. */
  value: string;
  /** Said under the saved device while it is not found; absent: nothing. */
  missing?: string;
  problem?: string;
  byHand: ReactNode;
  then: ReactNode;
  onPick: (value: string) => void;
}) {
  const discovery = use(DiscoveryContext);
  const scope = useFieldScopeContext();
  const nameId = useId();
  const legendId = useId();
  const lineId = useId();
  const { search, looking, look } = useCandidates(
    discovery,
    'capture',
    kind,
    field,
    true,
  );
  if (!discovery)
    return (
      <>
        {byHand}
        {problem ? (
          <p className="help" data-tone="warning" role="status">
            {problem}
          </p>
        ) : null}
        {then}
      </>
    );
  const found = 'found' in search ? search.found : [];
  // Until the first answer nothing is listed, not even the saved device:
  // which row is its row is known only then, so no row changes after.
  const answered = !(search.state === 'looking' && !search.again);
  /**
   * Nothing to pick once looked: typing is the way, so the field is not
   * tucked away (and stays put while looking again).
   */
  const plain = !found.length && answered;
  const listed = found.some((candidate) => candidate.fields[field] === value);
  const missingRow = answered && value !== '' && !listed;
  const rows = found.length > 0 || missingRow;
  const error = rows ? shownError(scope, path, false) : undefined;
  let status: string | null = null;
  if (error) status = error;
  else if (problem) status = problem;
  else if (looking) status = 'Looking for devices…';
  else if (search.state === 'failed')
    status = 'Couldn’t look for devices just now.';
  else if (!found.length)
    status = 'No devices found. Plug one in, or enter it below.';
  /**
   * A row says where its device is only when its name does not tell it
   * apart: two found under one name, or none; other details stay.
   */
  const aboutOf = (candidate: Candidate, at: string) => {
    const detail = detailOf(candidate);
    const named = candidate.label !== '';
    const shared =
      found.filter((other) => other.label === candidate.label).length > 1;
    // Without a name the path is the row's label already.
    return detail && (detail !== at || (shared && named)) ? detail : undefined;
  };
  const choose = (next: string) => {
    scope.edited(path);
    onPick(next);
  };
  return (
    <>
      <fieldset
        className="device-rows"
        role="radiogroup"
        aria-labelledby={legendId}
        aria-describedby={status ? lineId : undefined}
        aria-invalid={error ? true : undefined}
        data-field={rows ? path : undefined}
        tabIndex={rows ? -1 : undefined}
      >
        {/* The group is named by its label alone; Look again acts on it. */}
        <legend>
          <span id={legendId}>{legend}</span>
          <button
            type="button"
            className="link"
            disabled={scope.disabled}
            aria-disabled={looking || undefined}
            aria-busy={looking}
            onClick={() => look(discovery)}
          >
            Look again
          </button>
        </legend>
        {rows ? (
          <ul className="rows">
            {found.map((candidate) => {
              const at = candidate.fields[field] as string;
              return (
                <RowChoice
                  key={at}
                  name={nameId}
                  label={candidate.label || at}
                  about={aboutOf(candidate, at)}
                  checked={at === value}
                  disabled={scope.disabled}
                  onChoose={() => choose(at)}
                />
              );
            })}
            {missingRow ? (
              <MissingRow
                name={nameId}
                label={keptName('capture', { kind, [field]: value })}
                value={value}
                missing={missing}
              />
            ) : null}
          </ul>
        ) : null}
        <p
          id={lineId}
          className="help"
          role="status"
          data-tone={error || problem ? 'warning' : undefined}
        >
          {status}
        </p>
      </fieldset>
      {plain ? byHand : null}
      {then}
      {plain ? null : (
        // While there are rows, a problem of the device is theirs to say.
        <Advanced paths={rows ? [] : [path]}>{byHand}</Advanced>
      )}
    </>
  );
}

/** The saved device that is not found: by its kept name, else its value. */
function MissingRow({
  name,
  label,
  value,
  missing,
}: {
  name: string;
  label: string | null;
  value: string;
  missing?: string;
}) {
  return (
    <RowChoice
      name={name}
      label={label ?? value}
      about={label === null ? undefined : value}
      problem={missing}
      checked
      onChoose={() => {}}
    />
  );
}
