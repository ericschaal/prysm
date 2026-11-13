import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import {
  describeLighting,
  describeOutputIssue,
  failureOf,
  same,
  type Failure,
  type LightingTarget,
  type OutputIssue,
  type Session,
  plainFailure,
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

/**
 * The power button's busy ring: shown only once an operation has taken this
 * long (a quick one shows none), and then kept at least `ringHoldMs`, so it
 * never flickers.
 */
export const ringDelayMs = 350;
export const ringHoldMs = 450;

/** `active`, but only after `delay`, and then for at least `hold`. */
function useLingering(active: boolean, delay: number, hold: number) {
  const [shown, setShown] = useState(false);
  const shownAtRef = useRef(0);
  useEffect(() => {
    if (active === shown) return;
    const wait = active
      ? delay
      : Math.max(0, hold - (Date.now() - shownAtRef.current));
    const timer = setTimeout(() => {
      if (active) shownAtRef.current = Date.now();
      setShown(active);
    }, wait);
    return () => clearTimeout(timer);
  }, [active, shown, delay, hold]);
  return shown;
}

/** The card's one informative line: a problem, a warning or a note. */
interface Info {
  tone: 'problem' | 'warning' | 'note';
  text: string;
  /** Offer Try again: the requested mode failed. */
  retry?: boolean;
  /** The technical message, under Details. */
  details?: string;
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
  // In the card's order: a failure first, then what the session does for
  // the request (a session still in the previous mode is starting).
  if (session.state === 'faulted')
    return describeLighting({ target, session, failure, effects });
  if (target === 'off') return 'Lights are off';
  if (failure)
    return target === 'processed'
      ? 'Screen sync stopped'
      : `${effects} could not start`;
  if (target === 'processed' && session.state === 'running')
    return 'Lights follow your screen';
  if (target === 'device_native' && session.state === 'native')
    return `${effects} are on`;
  return target === 'processed'
    ? 'Starting screen sync…'
    : `Starting ${effects}…`;
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
  /** The latest command's own failure (a click that was refused). */
  const [error, setError] = useState<Failure | null>(null);
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
    setError(null);
    try {
      await setTarget(next);
    } catch (reason) {
      setError(failureOf(reason));
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

  // A failure wins, whatever the session still does: after a failed start
  // from effects, the engine leaves the session `native` (the lights keep
  // their own effects) with the target `processed` and the failure set.
  // Lit only when the session does what was asked.
  let mood = 'off';
  if (faulted || (on && failure)) mood = 'problem';
  else if (session.state === 'running' && target === 'processed') mood = 'sync';
  else if (session.state === 'native' && target === 'device_native')
    mood = 'effects';
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
  /** What went wrong: the latest command's own failure, else the state's. */
  // Faulted, the status says what to do instead.
  // A failure belongs to the lights being on; off, only a refused click counts.
  const problem = faulted ? null : (error ?? (on ? failure : null));

  // A pause worth naming, beside the eyebrow (never a line of its own, so
  // the card keeps its size): only once it lasts, so a normal start says
  // nothing.
  let pause: 'waiting' | 'starting' | null = null;
  if (input && session.state === 'running' && !input.receiving)
    pause = 'waiting';
  else if (mood === 'starting' && !lit) pause = 'starting';
  const [lasting, setLasting] = useState<typeof pause>(null);
  useEffect(() => {
    if (!pause) return;
    const timer = setTimeout(() => setLasting(pause), slowStartMs);
    return () => {
      clearTimeout(timer);
      setLasting(null);
    };
  }, [pause]);
  const note = lasting === pause ? pause : null;
  // Kept while it fades out, so it leaves as it came.
  const [keptNote, setKeptNote] = useState(note);
  if (note && note !== keptNote) setKeptNote(note);
  const status = headline({ target, session, failure, effects });
  let palette = mode === 'device_native' ? 'effects' : 'sync';
  if (mood === 'problem') palette = 'error';
  // Busy: a click on its way, or the lights starting (not yet lit).
  const ring = useLingering(
    pending !== null || (mood === 'starting' && !lit),
    ringDelayMs,
    ringHoldMs,
  );
  // The card's one line of words, only when it says what the card does not.
  let info: Info | null = null;
  if (problem)
    info = {
      tone: 'problem',
      text: plainFailure(problem, target, effects),
      details: problem.message,
      retry: !!failure && on,
    };
  else if (faulted) info = { tone: 'problem', text: status, retry: false };
  else if (outputIssue && session.state === 'running')
    info = { tone: 'warning', text: describeOutputIssue(outputIssue) };

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
    // The frame carries the light's colours, for the card and the LED drawing.
    <div
      className="light-frame"
      data-lit={lit}
      data-ignite={ignite}
      data-palette={palette}
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
          {/* Said by the status for screen readers; shown here, not twice. */}
          <span
            className="overline-note"
            data-shown={!!note}
            aria-hidden="true"
          >
            {' · '}
            {keptNote === 'waiting' ? 'Waiting for video' : 'Starting'}
          </span>
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
                data-ring={ring}
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
              {shown?.details || shown?.retry ? (
                // The fix and the details share one row.
                <div className="light-info-actions">
                  {shown.retry ? (
                    <button
                      type="button"
                      disabled={locked}
                      onClick={() => void choose(target)}
                    >
                      Try again
                    </button>
                  ) : null}
                  {shown.details ? (
                    <details className="light-details">
                      <summary>Details</summary>
                      <p>{shown.details}</p>
                    </details>
                  ) : null}
                </div>
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
    </div>
  );
}
