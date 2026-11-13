import { useEffect, useId, useState, type ReactNode } from 'react';
import {
  describeLighting,
  describeOutputIssue,
  failureOf,
  same,
  type Failure,
  type LightingTarget,
  type OutputIssue,
  type Session,
} from '@prysm/application-client';
import type { Input } from '@prysm/contracts/application';
import type { OutputLighting } from './adapters';

/** What the lights show while on: screen sync or the output's own effects. */
export type LightingMode = Exclude<LightingTarget, 'off'>;

export interface LightingProps {
  target: LightingTarget;
  session: Session;
  failure: Failure | null;
  /** Why the lights are not showing screen sync while it runs. */
  outputIssue?: OutputIssue;
  /** Whether frames arrive, while screen sync runs. */
  input?: Input;
  hasCapture: boolean;
  hasOutput: boolean;
  /** What the configured output offers, `null` without an output. */
  output: OutputLighting | null;
  disabled: boolean;
  setTarget: (target: LightingTarget) => Promise<unknown>;
  /** Opens the guided setup; offered while capture or lights are missing. */
  openSetup: () => void;
  /** The screen sync picture controls, under the mode tiles. */
  children?: ReactNode;
}

/**
 * The mode the power button turns on: a per-browser preference, like the
 * theme. Prysm itself only knows the current target.
 */
export const modeStorageKey = 'prysm.mode';

function storedMode(): LightingMode {
  try {
    return window.localStorage.getItem(modeStorageKey) === 'device_native'
      ? 'device_native'
      : 'processed';
  } catch {
    return 'processed';
  }
}

function saveMode(mode: LightingMode) {
  try {
    window.localStorage.setItem(modeStorageKey, mode);
  } catch {
    // Without storage the power button turns on screen sync next time.
  }
}

/** The state words, drawn in one place and cross-faded. */
const words = ['Off', 'On', 'Stopped'] as const;
type Word = (typeof words)[number];

/** Starting is mentioned only once it takes longer than this. */
export const slowStartMs = 1000;

/** The card's one informative line: a problem, a warning or a note. */
interface Info {
  tone: 'problem' | 'warning' | 'note';
  text: string;
  /** Offer Try again: the requested mode failed. */
  retry?: boolean;
}

/** Why screen sync cannot start yet, or `null`. */
function missingSetup(hasCapture: boolean, hasOutput: boolean) {
  if (!hasCapture && !hasOutput)
    return 'Set up a video source and lights first.';
  if (!hasCapture) return 'Set up a video source first.';
  if (!hasOutput) return 'Set up your lights first.';
  return null;
}

/** Why the output's own effects cannot start, or `null`. */
function missingEffects(hasOutput: boolean, effects: string | undefined) {
  if (!hasOutput) return 'Set up your lights first.';
  if (!effects) return 'These lights have no effects of their own.';
  return null;
}

/** The state in plain words, under the state word. */
function headline({
  target,
  session,
  failure,
  effects,
}: {
  target: LightingTarget;
  session: Session;
  failure: Failure | null;
  effects: string;
}) {
  switch (session.state) {
    case 'faulted':
      return describeLighting({ target, session, failure, effects });
    case 'running':
      return 'Lights follow your screen';
    case 'native':
      return `${effects} are on`;
    case 'stopped':
      if (target === 'off') return 'Lights are off';
      if (failure)
        return target === 'processed'
          ? 'Screen sync stopped'
          : `${effects} could not start`;
      return target === 'processed'
        ? 'Starting screen sync…'
        : `Starting ${effects}…`;
  }
}

function PowerIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M12 3.5v8M6.6 6.2a7.6 7.6 0 1 0 10.8 0" />
    </svg>
  );
}

/** A screen throwing its two halves' colours onto the wall beside it. */
function SyncArt() {
  return (
    <svg
      className="mode-art"
      viewBox="0 0 64 48"
      aria-hidden="true"
      focusable="false"
    >
      <path
        className="art-warm"
        d="M32 10H17a3 3 0 0 0-3 3v17a3 3 0 0 0 3 3h15"
      />
      <path
        className="art-cool"
        d="M32 10h15a3 3 0 0 1 3 3v17a3 3 0 0 1-3 3H32"
      />
      <path className="art-line" d="M26 40h12M32 33v7" />
      <path className="art-warm art-led" d="M7 15v13" />
      <path className="art-cool art-led" d="M57 15v13" />
    </svg>
  );
}

/** Light moving on its own: three waves, the middle one lit. */
function EffectsArt() {
  return (
    <svg
      className="mode-art"
      viewBox="0 0 64 48"
      aria-hidden="true"
      focusable="false"
    >
      <path className="art-line" d="M10 24c11-26 27 12 44-17" />
      <path className="art-line" d="M10 42c11-26 27 12 44-17" />
      <path className="art-glow" d="M10 33c11-26 27 12 44-17" />
    </svg>
  );
}

/**
 * Your lights: the state as one word, the power button, the two modes as
 * illustrated tiles and the picture controls, over a light that drifts
 * while the lights are on. Each control sets a `LightingTarget`; the state
 * stream, not the click, decides what is shown as on.
 */
export function LightCard({
  target,
  session,
  failure,
  outputIssue,
  input,
  hasCapture,
  hasOutput,
  output,
  disabled,
  setTarget,
  openSetup,
  children,
}: LightingProps) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const [error, setError] = useState('');
  const [remembered, setRemembered] = useState(storedMode);
  if (target !== 'off' && target !== remembered) setRemembered(target);
  useEffect(() => {
    if (target !== 'off') saveMode(target);
  }, [target]);
  const headingId = useId();
  const statusId = useId();
  const effects = output?.effects ?? 'Device effects';
  const faulted = session.state === 'faulted';
  const on = target !== 'off';
  const configured = hasCapture && hasOutput;

  async function choose(next: LightingTarget) {
    setPending(next);
    setError('');
    try {
      await setTarget(next);
    } catch (reason) {
      setError(failureOf(reason).message);
    } finally {
      setPending(null);
    }
  }

  const reasonOf = (unavailable: string | null) =>
    faulted ? 'Restart Prysm to change the lights.' : unavailable;
  const reasons: Record<LightingMode, string | null> = {
    processed: reasonOf(missingSetup(hasCapture, hasOutput)),
    device_native: reasonOf(missingEffects(hasOutput, output?.effects)),
  };
  // The chosen mode: the one on, else the one the power button turns on.
  let mode: LightingMode = on ? target : remembered;
  const other = mode === 'processed' ? 'device_native' : 'processed';
  if (!on && reasons[mode] && !reasons[other]) mode = other;
  const powerReason = on ? reasonOf(null) : reasons[mode];
  const locked = disabled || pending !== null;

  let mood = 'off';
  if (session.state === 'running') mood = 'sync';
  else if (session.state === 'native') mood = 'effects';
  else if (faulted || (on && failure)) mood = 'problem';
  else if (on) mood = 'starting';
  if (!configured) mood = 'unset';
  // Switching modes passes through starting: the light stays up meanwhile.
  const [stayLit, setStayLit] = useState(false);
  const running = mood === 'sync' || mood === 'effects';
  if (running && !stayLit) setStayLit(true);
  if (!running && mood !== 'starting' && stayLit) setStayLit(false);
  const lit = running || (mood === 'starting' && stayLit);
  // Each time the light comes on (not on load), the ignition plays once. Its
  // CSS animations alternate between two names, so a new one restarts them
  // without remounting anything.
  const [wasLit, setWasLit] = useState(lit);
  const [ignitions, setIgnitions] = useState(0);
  if (lit !== wasLit) {
    setWasLit(lit);
    if (lit) setIgnitions(ignitions + 1);
  }
  let ignite: 'a' | 'b' | undefined;
  if (ignitions) ignite = ignitions % 2 ? 'a' : 'b';
  // The word follows the light, not the request: "On" arrives with the light.
  let word: Word = lit ? 'On' : 'Off';
  if (mood === 'problem') word = 'Stopped';
  /** What went wrong, in one sentence: the latest command's error, else the failure. */
  // Faulted, the status says what to do instead.
  const problem = faulted ? '' : error || failure?.message || '';

  // Starting is said only when it lasts.
  const [slowStart, setSlowStart] = useState(false);
  useEffect(() => {
    if (mood !== 'starting') return;
    const timer = setTimeout(() => setSlowStart(true), slowStartMs);
    return () => {
      clearTimeout(timer);
      setSlowStart(false);
    };
  }, [mood]);
  const status = headline({ target, session, failure, effects });
  // The card's one line of words, only when it says what the card does not.
  let info: Info | null = null;
  if (problem)
    info = { tone: 'problem', text: problem, retry: !!failure && on };
  else if (faulted) info = { tone: 'problem', text: status, retry: false };
  else if (outputIssue && session.state === 'running')
    info = { tone: 'warning', text: describeOutputIssue(outputIssue) };
  else if (input && session.state === 'running' && !input.receiving)
    info = { tone: 'note', text: 'Waiting for video.' };
  else if (mood === 'starting' && slowStart && !lit)
    info = { tone: 'note', text: status };
  // Kept while the line folds away, so it leaves as it came.
  const [kept, setKept] = useState(info);
  if (info && !same(info, kept)) setKept(info);
  const shown = info ?? kept;

  const tiles = [
    { value: 'processed', label: 'Screen sync', art: <SyncArt /> },
    { value: 'device_native', label: effects, art: <EffectsArt /> },
  ] as const;
  // One visible line per distinct reason; every tile keeps its own for AT.
  const shownReasons = new Set<string>();

  return (
    // The frame carries the light's colours to the glow under the card.
    <div
      className="light-frame"
      data-lit={lit}
      data-ignite={ignite}
      data-palette={mode === 'device_native' ? 'effects' : 'sync'}
    >
      <section
        className="light-card"
        data-mood={mood}
        data-mode={mode}
        data-lit={lit}
        data-ignite={ignite}
        aria-labelledby={headingId}
        aria-busy={pending !== null}
      >
        <div className="aurora" aria-hidden="true">
          <div className="aurora-light">
            <span />
            <span />
            <span />
            <span />
          </div>
          {/* The ignition: a brief lift of the light, and its leading edge. */}
          <div className="aurora-crest" />
          <div className="aurora-wave" />
        </div>
        <h2 id={headingId} className="overline">
          Your lights
        </h2>
        <div className="light-head">
          {/* The three words share one cell and cross-fade. */}
          <p className="light-word" aria-hidden="true">
            {words.map((each) => (
              <span key={each} data-word={each} data-shown={each === word}>
                {each}
              </span>
            ))}
          </p>
          {configured ? (
            <div className="power-wrap">
              <button
                type="button"
                className="power"
                data-power=""
                aria-label="Power"
                aria-pressed={on}
                aria-busy={pending !== null && (pending === 'off') === on}
                aria-describedby={
                  powerReason
                    ? `${headingId}-power`
                    : `${statusId} ${headingId}-starts`
                }
                disabled={locked || powerReason !== null}
                data-unavailable={powerReason !== null}
                onClick={() => void choose(on ? 'off' : mode)}
              >
                <PowerIcon />
              </button>
            </div>
          ) : null}
        </div>
        {/* For screen readers; sighted people see the word and the chosen tile. */}
        <p
          id={statusId}
          className="lighting-status visually-hidden"
          role="status"
          aria-live="polite"
        >
          {status}
        </p>
        {powerReason ? (
          <small id={`${headingId}-power`} className="visually-hidden">
            {powerReason}
          </small>
        ) : (
          <small id={`${headingId}-starts`} className="visually-hidden">
            {on
              ? ''
              : `Turns on ${mode === 'processed' ? 'Screen sync' : effects}.`}
          </small>
        )}
        {configured ? (
          // Opens and closes its own space, so nothing below jumps.
          <div
            className="light-info"
            data-shown={!!info}
            data-tone={shown?.tone}
            inert={!info}
          >
            <div>
              {shown ? (
                <p role={shown.tone === 'problem' ? 'alert' : 'status'}>
                  {shown.text}
                </p>
              ) : null}
              {shown?.retry ? (
                <button
                  type="button"
                  disabled={locked}
                  onClick={() => void choose(target)}
                >
                  Try again
                </button>
              ) : null}
            </div>
          </div>
        ) : (
          <div className="light-setup">
            <p>Prysm needs a video source and lights before it can light up.</p>
            <button type="button" className="primary" onClick={openSetup}>
              Set up Prysm
            </button>
          </div>
        )}
        {configured ? (
          <>
            <div
              className="modes"
              role="group"
              aria-label="Lighting mode"
              data-mode={mode}
              data-on={on}
            >
              {tiles.map((tile) => (
                <button
                  type="button"
                  key={tile.value}
                  className="mode"
                  data-target={tile.value}
                  aria-pressed={target === tile.value}
                  aria-busy={pending === tile.value}
                  aria-describedby={
                    reasons[tile.value]
                      ? `${headingId}-${tile.value}`
                      : undefined
                  }
                  disabled={locked || reasons[tile.value] !== null}
                  // Only a mode that cannot run looks disabled; a click in flight does not.
                  data-unavailable={reasons[tile.value] !== null}
                  onClick={() => {
                    // Already on and fine: nothing to restart. After a failure it retries.
                    if (target !== tile.value || failure)
                      void choose(tile.value);
                  }}
                >
                  {tile.art}
                  <span className="mode-label">{tile.label}</span>
                </button>
              ))}
            </div>
            {tiles.map(({ value }) => {
              const reason = reasons[value];
              if (!reason) return null;
              const repeated = shownReasons.has(reason);
              shownReasons.add(reason);
              return (
                <small
                  key={value}
                  id={`${headingId}-${value}`}
                  className={repeated ? 'visually-hidden' : 'mode-reason'}
                >
                  {reason}
                </small>
              );
            })}
            {/* Both panels share one cell, so switching modes never moves the page. */}
            <div className="mode-panels">
              <div className="mode-panel" inert={mode !== 'processed'}>
                {children}
              </div>
              <div
                className="mode-panel effects-panel"
                inert={mode !== 'device_native'}
              >
                <p>
                  {effects} run on the lights themselves. Choose an effect and
                  its brightness there.
                </p>
                {output?.link ? (
                  <a
                    className="device-link"
                    href={output.link.href}
                    target="_blank"
                    rel="noopener noreferrer"
                  >
                    {output.link.label}
                    <span className="visually-hidden">
                      {' '}
                      (opens in a new tab)
                    </span>
                  </a>
                ) : null}
              </div>
            </div>
          </>
        ) : null}
      </section>
      <span className="light-spill" aria-hidden="true" />
    </div>
  );
}
