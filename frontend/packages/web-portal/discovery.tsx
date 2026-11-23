import { use, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import {
  type AdapterSide,
  type Candidate,
  type DiscoveryClient,
} from '@prysm/application-client';
import { keepNames, keptName } from './device-name';
import { DiscoveryContext } from './discovery-context';
import { Advanced, Note } from './fields';
import { shownError, useCheck, useFieldScopeContext } from './form';
import { CheckIcon, RowChoice } from './rows';

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
 * Ask the adapter for `kind` what it finds, at once and again on `look`
 * (showing the last ones meanwhile), keeping the candidates that fill
 * `field`, and keep their names in this browser. A lookup the editor no
 * longer shows is ended.
 */
function useCandidates(
  discovery: DiscoveryClient | null,
  section: AdapterSide,
  kind: string,
  field: string,
) {
  const [search, setSearch] = useState<Search>(
    discovery
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
    if (discovery) void askRef.current(discovery);
    return () => abortRef.current?.abort();
  }, [discovery]);
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

/** The words of the row to type in. */
export interface TypedRow {
  /** The radio's name, such as `Another device`. */
  choice: string;
  /** The field's name, such as `Device path`. */
  label: string;
  placeholder: string;
  /** Said when nothing is picked or typed. */
  empty: string;
}

/**
 * The devices of `kind` present (capture devices, WLED lights), looked for
 * as the editor opens, as rows to tick, listed once the first lookup has
 * answered and kept while looking again; picking one fills `path` (its
 * field at `field`). "Look again" sits at the end of the heading line.
 * The last row is always one to type in (`typed`), picked by focusing or
 * typing in its field; it keeps what was typed while a found row is
 * picked. A saved value the lookup does not list is in it, with the name
 * this browser last saw for it and `missing` beneath. That row says the
 * problem of `path` beneath it and takes the focus for it, unless a found
 * row is picked: then the line under the rows says it, as it says how the
 * lookup went (looking, nothing found, failed) or `problem` (what is wrong
 * with the picked device). Then come what follows the device (`then`) and
 * the editor's advanced fields (`advanced`) under Advanced.
 */
export function DeviceRows({
  section,
  kind,
  path,
  field,
  legend,
  things,
  nothing,
  value,
  text,
  typed,
  missing,
  problem,
  then,
  advanced,
  onPick,
  onType,
  onLook,
}: {
  section: AdapterSide;
  kind: string;
  path: string;
  field: string;
  legend: string;
  /** What is looked for, such as `devices`. */
  things: string;
  /** Said when nothing is found, with what to do instead. */
  nothing: string;
  /** The value as the rows compare it, such as the device path; `''`: none. */
  value: string;
  /** The value as typed. */
  text: string;
  typed: TypedRow;
  /** Said under a typed value while it is not found; absent: nothing. */
  missing?: string;
  problem?: string;
  then: ReactNode;
  /** Fields under Advanced, and their paths. */
  advanced?: { paths: readonly string[]; fields: ReactNode };
  onPick: (value: string, candidate: Candidate) => void;
  onType: (text: string) => void;
  /** Called on each Look again, so what follows from the device is too. */
  onLook?: () => void;
}) {
  const discovery = use(DiscoveryContext);
  const scope = useFieldScopeContext();
  const nameId = useId();
  const legendId = useId();
  const lineId = useId();
  const { search, looking, look } = useCandidates(
    discovery,
    section,
    kind,
    field,
  );
  /**
   * The text last typed or picked in the typed row: while it is the value,
   * that row stays picked, even for a found value typed out.
   */
  const [mine, setMine] = useState<string | null>(null);
  /** What was typed, kept while a found row is picked. */
  const [draft, setDraft] = useState('');
  const [touched, setTouched] = useState(false);
  useCheck(path, value ? null : typed.empty);
  const found = 'found' in search ? search.found : [];
  // Until the first answer no row is listed, not even the typed one: which
  // row is the saved value's is known only then, so no row changes after.
  const answered = !(search.state === 'looking' && !search.again);
  const listed = found.some((candidate) => candidate.fields[field] === value);
  const typedPicked =
    mine === text || (!listed && (value !== '' || found.length === 0));
  // The typed row says the problem of `path` and takes the focus for it,
  // unless a found row is picked.
  const inRow = answered && (typedPicked || !listed);
  const error = shownError(scope, path, touched);
  const lineError = inRow ? undefined : error;
  let status: string | null = null;
  if (lineError) status = lineError;
  else if (problem) status = problem;
  else if (looking) status = `Looking for ${things}…`;
  else if (search.state === 'failed')
    status = `Couldn’t look for ${things} just now.`;
  else if (search.state === 'found' && !found.length) status = nothing;
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
  const pick = (next: string, candidate: Candidate) => {
    if (typedPicked) setDraft(text);
    setMine(null);
    scope.edited(path);
    onPick(next, candidate);
  };
  const type = (next: string) => {
    setMine(next);
    scope.edited(path);
    onType(next);
  };
  // Picking the typed row brings back what was typed; nothing else changes.
  const pickTyped = () => {
    if (typedPicked) return;
    if (draft === text) setMine(text);
    else type(draft);
  };
  const retry =
    inRow && scope.retry?.path === path ? scope.retry.run : undefined;
  const shown = typedPicked && value !== '';
  const kept = shown ? keptName(section, { kind, [field]: value }) : null;
  const gone = shown ? missing : undefined;
  return (
    <>
      <fieldset
        className="device-rows"
        role="radiogroup"
        aria-labelledby={legendId}
        aria-describedby={status ? lineId : undefined}
        aria-invalid={lineError ? true : undefined}
        data-field={inRow ? undefined : path}
        tabIndex={inRow ? undefined : -1}
      >
        {/* The group is named by its label alone; Look again acts on it. */}
        <legend>
          <span id={legendId}>{legend}</span>
          {discovery ? (
            <button
              type="button"
              className="link"
              disabled={scope.disabled}
              aria-disabled={looking || undefined}
              aria-busy={looking}
              onClick={() => {
                look(discovery);
                onLook?.();
              }}
            >
              Look again
            </button>
          ) : null}
        </legend>
        {answered ? (
          <ul className="rows">
            {found.map((candidate) => {
              const at = candidate.fields[field] as string;
              return (
                <RowChoice
                  key={at}
                  name={nameId}
                  label={candidate.label || at}
                  about={aboutOf(candidate, at)}
                  checked={!typedPicked && at === value}
                  disabled={scope.disabled}
                  onChoose={() => pick(at, candidate)}
                />
              );
            })}
            <RowTyped
              name={nameId}
              path={path}
              typed={typed}
              text={typedPicked ? text : draft}
              checked={typedPicked}
              disabled={scope.disabled}
              line={
                kept || gone ? (
                  <>
                    {kept}
                    {kept && gone ? ' · ' : null}
                    {gone ? <span className="row-problem">{gone}</span> : null}
                  </>
                ) : undefined
              }
              error={inRow ? error : undefined}
              retry={retry}
              onPick={pickTyped}
              onType={type}
              onBlur={() => setTouched(true)}
            />
          </ul>
        ) : null}
        <p
          id={lineId}
          className="help"
          role="status"
          data-tone={lineError || problem ? 'warning' : undefined}
        >
          {status}
        </p>
      </fieldset>
      {then}
      {advanced ? (
        <Advanced paths={advanced.paths}>{advanced.fields}</Advanced>
      ) : null}
    </>
  );
}

/**
 * The row to type in: a radio as the others are, its field in the label's
 * place and the ring in its own. Focusing the field picks the row; the
 * field is in the tab order only while the row is picked, so tabbing past
 * the choice never changes it, and in the field the arrow keys move the
 * caret. Its help (`line`) or its problem (`error`) is on the line beneath,
 * as under any field.
 */
function RowTyped({
  name,
  path,
  typed,
  text,
  checked,
  disabled,
  line,
  error,
  retry,
  onPick,
  onType,
  onBlur,
}: {
  name: string;
  path: string;
  typed: TypedRow;
  text: string;
  checked: boolean;
  disabled: boolean;
  line?: ReactNode;
  error?: string;
  retry?: () => void;
  onPick: () => void;
  onType: (text: string) => void;
  onBlur: () => void;
}) {
  const id = useId();
  let described: string | undefined;
  if (error) described = `${id}-error`;
  else if (line) described = `${id}-help`;
  return (
    <li className="row row-choice row-typed">
      <label>
        <input
          type="radio"
          name={name}
          aria-label={typed.choice}
          aria-describedby={described}
          checked={checked}
          disabled={disabled}
          onChange={onPick}
        />
        <span className="field">
          <input
            type="text"
            id={id}
            name={path}
            aria-label={typed.label}
            aria-invalid={error ? true : undefined}
            aria-describedby={described}
            value={text}
            placeholder={typed.placeholder}
            disabled={disabled}
            tabIndex={checked ? undefined : -1}
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            onFocus={onPick}
            onChange={(event) => onType(event.currentTarget.value)}
            onBlur={onBlur}
          />
          <Note id={id} help={line} error={error} retry={retry} />
        </span>
        <CheckIcon />
      </label>
    </li>
  );
}
