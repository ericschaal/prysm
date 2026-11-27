import { use, useEffect, useId, useRef, useState } from 'react';
import {
  type AdapterSide,
  type Candidate,
  type DiscoveryClient,
} from '@prysm/application-client';
import { keepNames, keptName } from './device-name';
import { HostContext } from './portal-context';
import { Note } from './fields';
import { shownError, useCheck, useFieldScopeContext } from './form';
import { CheckIcon, RowChoice } from './rows';

type CandidateRow = { candidate: Candidate; at: string };

type Search =
  | { state: 'idle' | 'looking' | 'failed' }
  | { state: 'refreshing' | 'found'; found: CandidateRow[] };

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
function useCandidates(section: AdapterSide, kind: string, field: string) {
  const discovery = use(HostContext)?.discovery;
  const [search, setSearch] = useState<Search>({
    state: discovery ? 'looking' : 'idle',
  });
  const abortRef = useRef<AbortController | null>(null);
  async function ask(client: DiscoveryClient) {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    try {
      const found = await client.candidates(section, kind, controller.signal);
      if (controller.signal.aborted) return;
      const usable = found.flatMap((candidate) => {
        const at = candidate.fields[field];
        return typeof at === 'string' ? [{ candidate, at }] : [];
      });
      keepNames(
        section,
        kind,
        usable.map(({ candidate }) => candidate),
      );
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
  const looking = search.state === 'looking' || search.state === 'refreshing';
  /** Ask again; ignored while asking, so the control keeps its focus. */
  function look() {
    if (!discovery || looking) return;
    setSearch({
      state: 'refreshing',
      found: search.state === 'found' ? search.found : [],
    });
    void ask(discovery);
  }
  return { search, looking, look: discovery ? look : undefined };
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
 * Choose a discovered device or enter its address in the manual row. The first
 * lookup disables selection. Looking again keeps the previous choices and the
 * manual draft. Field errors appear on the manual row or the group, whichever
 * accepts focus for the current choice.
 */
export function DeviceRows({
  section,
  kind,
  field,
  legend,
  things,
  nothing,
  value,
  text,
  typed,
  missing,
  problem,
  onPick,
  onType,
  onLook,
}: {
  section: AdapterSide;
  kind: string;
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
  onPick: (value: string, candidate: Candidate) => void;
  onType: (text: string) => void;
  /** Called on each Look again, so what follows from the device is too. */
  onLook?: () => void;
}) {
  const path = `${section}.${field}`;
  const scope = useFieldScopeContext();
  const nameId = useId();
  const typedId = useId();
  const legendId = useId();
  const lineId = useId();
  const { search, looking, look } = useCandidates(section, kind, field);
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
  // Until the first answer no row is picked or left: which row is the saved
  // value's is known only then.
  const answered = search.state !== 'looking';
  const listed = found.some(({ at }) => at === value);
  const typedPicked =
    mine === text || (!listed && (value !== '' || found.length === 0));
  // The typed row says the problem of `path` and takes the focus for it,
  // unless a found row is picked.
  const inRow = answered && (typedPicked || !listed);
  const error = shownError(scope, path, touched);
  const lineError = inRow ? undefined : error;
  const lookingFor = `Looking for ${things}…`;
  let status: string | null = null;
  if (lineError) status = lineError;
  else if (problem) status = problem;
  else if (search.state === 'refreshing') status = lookingFor;
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
      found.filter(({ candidate: other }) => other.label === candidate.label)
        .length > 1;
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
  const typedShown = answered && typedPicked;
  const savedName = value ? keptName(section, { kind, [field]: value }) : null;
  const shown = typedShown && value !== '';
  const kept = shown ? savedName : null;
  const gone = shown ? missing : undefined;
  const typedError = inRow ? error : undefined;
  const typedHelp =
    kept || gone ? (
      <>
        {kept}
        {kept && gone ? ' · ' : null}
        {gone ? <span className="row-problem">{gone}</span> : null}
      </>
    ) : undefined;
  const typedDisabled = scope.disabled || !answered;
  let described: string | undefined;
  if (typedError) described = `${typedId}-error`;
  else if (typedHelp) described = `${typedId}-help`;

  return (
    <fieldset
      className="device-rows"
      role="radiogroup"
      aria-labelledby={legendId}
      aria-describedby={status ? lineId : undefined}
      aria-busy={!answered}
      aria-invalid={lineError ? true : undefined}
      data-field={inRow ? undefined : path}
      tabIndex={inRow ? undefined : -1}
    >
      {/* The group is named by its label alone; Look again acts on it. */}
      <legend>
        <span id={legendId}>{legend}</span>
        {look ? (
          <button
            type="button"
            className="link"
            disabled={scope.disabled}
            aria-disabled={looking || undefined}
            aria-busy={looking}
            onClick={() => {
              look();
              onLook?.();
            }}
          >
            Look again
          </button>
        ) : null}
      </legend>
      <ul className="rows">
        {answered ? (
          found.map(({ candidate, at }) => (
            <RowChoice
              key={at}
              name={nameId}
              label={candidate.label || at}
              about={aboutOf(candidate, at)}
              checked={!typedPicked && at === value}
              disabled={scope.disabled}
              onChoose={() => pick(at, candidate)}
            />
          ))
        ) : (
          <RowChoice
            name={nameId}
            label={value ? (savedName ?? value) : lookingFor}
            about={value ? lookingFor : undefined}
            checked={value !== ''}
            disabled
          />
        )}
        {/* Focus picks the typed row; unselected, its textbox is outside the tab order. */}
        <li className="row row-choice row-typed">
          <label>
            <input
              type="radio"
              name={nameId}
              aria-label={typed.choice}
              aria-describedby={described}
              checked={typedShown}
              disabled={typedDisabled}
              onChange={pickTyped}
            />
            <span className="field">
              <input
                type="text"
                id={typedId}
                name={path}
                aria-label={typed.label}
                aria-invalid={typedError ? true : undefined}
                aria-describedby={described}
                value={typedShown ? text : draft}
                placeholder={typed.placeholder}
                disabled={typedDisabled}
                tabIndex={typedShown ? undefined : -1}
                autoComplete="off"
                autoCapitalize="off"
                spellCheck={false}
                onFocus={pickTyped}
                onChange={(event) => type(event.currentTarget.value)}
                onBlur={() => setTouched(true)}
              />
              <Note
                id={typedId}
                help={typedHelp}
                error={typedError}
                retry={retry}
              />
            </span>
            <CheckIcon />
          </label>
        </li>
      </ul>
      <p
        id={lineId}
        className="help"
        role="status"
        data-tone={lineError || problem ? 'warning' : undefined}
      >
        {status}
      </p>
    </fieldset>
  );
}
