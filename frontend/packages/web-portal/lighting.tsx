import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
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
import type { DeviceBrightness, Input } from '@prysm/contracts/application';
import type { OutputLighting } from './adapters';

/** What the lights show while on: screen sync or the output's own effects. */
export type LightingMode = Exclude<LightingTarget, 'off'>;

export interface LightingProps {
  target: LightingTarget;
  session: Session;
  failure: Failure | null;
  /** Why the lights are not showing screen sync while it runs. */
  outputIssue?: OutputIssue;
  /** The lights' own brightness while it scales screen sync's colours. */
  outputBrightness?: DeviceBrightness;
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
 * Below this share the lights' own brightness is named on the card. The
 * lights show Prysm's brightness times theirs; above about three quarters
 * the difference is hard to see (perceived lightness stays near 90 %), while
 * WLED's own default (128 of 255, 50 %) halves every colour unexplained.
 */
export const deviceBrightnessNoteBelow = 75;

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

/**
 * What the card shows: lit by screen sync or the effects, starting, stopped
 * by a problem, off, or without the setup it needs.
 */
type Mood = 'off' | 'problem' | 'sync' | 'effects' | 'starting' | 'unset';

/**
 * A failure wins, whatever the session still does: after a failed start
 * from effects, the engine leaves the session `native` (the lights keep
 * their own effects) with the target `processed` and the failure set. Lit
 * only when the session does what was asked.
 */
function moodOf(
  target: LightingTarget,
  session: Session,
  failure: Failure | null,
  configured: boolean,
): Mood {
  if (!configured) return 'unset';
  if (session.state === 'faulted' || (target !== 'off' && failure))
    return 'problem';
  if (session.state === 'running' && target === 'processed') return 'sync';
  if (session.state === 'native' && target === 'device_native')
    return 'effects';
  return target !== 'off' ? 'starting' : 'off';
}

/**
 * The calm line, said for sighted people (the status says it for screen
 * readers): what Power will do, a pause once it lasts, or what runs. Empty
 * while the card has something else to say, or Power cannot turn on.
 */
function calmLine({
  info,
  powerReason,
  target,
  session,
  input,
  mode,
  effects,
  note,
  lit,
}: {
  info: Info | null;
  powerReason: string | null;
  target: LightingTarget;
  session: Session;
  input: Input | undefined;
  mode: LightingMode;
  effects: string;
  note: 'waiting' | 'starting' | null;
  lit: boolean;
}) {
  const on = target !== 'off';
  if (info || (!on && powerReason)) return '';
  if (!on)
    return mode === 'processed'
      ? 'Power turns on Screen sync.'
      : `Power turns on ${effects}.`;
  if (note === 'waiting') return 'Waiting for the picture.';
  if (note === 'starting') return 'Starting…';
  if (lit && target === 'processed' && input?.receiving)
    return 'Following the screen.';
  if (target === 'device_native' && session.state === 'native')
    return `${effects} are showing.`;
  return '';
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

/** Screen sync: a screen with light at its sides. */
function SyncIcon() {
  return (
    <svg
      className="mode-icon"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <rect x="5.5" y="5" width="13" height="9.5" rx="1.5" />
      <path d="M9.5 19h5M12 14.5V19M2.5 7.5v5M21.5 7.5v5" />
    </svg>
  );
}

/** The lights' own effects: light moving by itself. */
function EffectsIcon() {
  return (
    <svg
      className="mode-icon"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M3 9.5c3-4 6 3 9 0s6-4 9 0M3 15.5c3-4 6 3 9 0s6-4 9 0" />
    </svg>
  );
}

/**
 * What a mode does, in a few words that fit one line in a half-width card on
 * a phone: never cut, so a long device name gets the general line.
 */
function effectsLine(effects: string | undefined) {
  const device = effects?.replace(/ effects$/, '');
  return device && device !== effects && device.length <= 8
    ? `Patterns from ${device}`
    : 'Its own patterns';
}

/**
 * Your lights: the state as one word and the power button, then three
 * labelled controls (what the lights follow, brightness, colour profile),
 * over a light that drifts while the lights are on. Each control sets a `LightingTarget`; the state
 * stream, not the click, decides what is shown as on.
 */
export function LightCard({
  target,
  session,
  failure,
  outputIssue,
  outputBrightness,
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

  const mood = moodOf(target, session, failure, configured);
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
  const status = headline({ target, session, failure, effects });
  let palette: 'sync' | 'effects' | 'error' =
    mode === 'device_native' ? 'effects' : 'sync';
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
  // Screen sync shows at the lights' own brightness too: a low one is named
  // in place of "Following the screen.", so Prysm at 100 % that looks dim
  // has its reason. Prysm never changes the lights' setting.
  else if (
    outputBrightness &&
    outputBrightness.percent < deviceBrightnessNoteBelow &&
    lit &&
    target === 'processed' &&
    session.state === 'running' &&
    input?.receiving
  )
    info = {
      tone: 'note',
      text: `${output?.effects?.replace(/ effects$/, '') ?? 'The lights'} limits brightness to ${outputBrightness.percent}%.`,
    };

  const calm = calmLine({
    info,
    powerReason,
    target,
    session,
    input,
    mode,
    effects,
    note,
    lit,
  });
  // With the lights' own effects chosen, Prysm's picture controls do not
  // reach the lights (the engine runs no session): one line says so, with
  // the way to the device's own controls.
  const ownEffects = mode === 'device_native' && !info && !(on && note);

  // Kept while the line folds away, so it leaves as it came.
  const [kept, setKept] = useState(info);
  if (info && !same(info, kept)) setKept(info);
  const shown = info ?? kept;

  const tiles = [
    {
      value: 'processed',
      label: 'Screen sync',
      line: 'Follows the screen',
      icon: <SyncIcon />,
    },
    {
      value: 'device_native',
      label: effects,
      // Named by the output when it can be: "Patterns from WLED".
      line: effectsLine(output?.effects),
      icon: <EffectsIcon />,
    },
  ] as const;
  /** Arrow keys move the choice, as in any radio group. */
  function arrow(event: KeyboardEvent<HTMLButtonElement>) {
    const steps: Record<string, number> = {
      ArrowRight: 1,
      ArrowDown: 1,
      ArrowLeft: -1,
      ArrowUp: -1,
    };
    const step = steps[event.key];
    if (!step) return;
    event.preventDefault();
    const index = tiles.findIndex((tile) => tile.value === mode);
    const next = tiles[(index + step + tiles.length) % tiles.length];
    if (!next || locked || reasons[next.value]) return;
    void choose(next.value);
    event.currentTarget.parentElement
      ?.querySelector<HTMLButtonElement>(`[data-target="${next.value}"]`)
      ?.focus();
  }
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
          // Its space is kept in every state: a problem, a warning or the
          // calm line cross-fade in it, so nothing below ever moves.
          <div
            className="light-info"
            data-shown={!!info}
            data-tone={shown?.tone}
          >
            <p className="light-calm" data-shown={!info}>
              {/* Said by the status for screen readers. */}
              <span aria-hidden="true">
                {ownEffects && (on || !calm)
                  ? `${effects.replace(/ effects$/, '')} controls its own effects.`
                  : calm}
              </span>
              {ownEffects && output?.link ? (
                <>
                  {' '}
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
                </>
              ) : null}
            </p>
            <div inert={!info}>
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
            {/* The mode: two option cards, each saying what it does. */}
            <div
              className="modes"
              role="radiogroup"
              aria-label="Mode"
              data-mode={mode}
              data-on={on}
            >
              {tiles.map((tile) => (
                <button
                  type="button"
                  role="radio"
                  key={tile.value}
                  className="mode"
                  data-target={tile.value}
                  aria-checked={mode === tile.value}
                  tabIndex={mode === tile.value ? 0 : -1}
                  onKeyDown={arrow}
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
                  {tile.icon}
                  <span className="mode-name">{tile.label}</span>
                  <span className="mode-line">{tile.line}</span>
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
            {/* Prysm's picture controls: always in place, quiet (and out of
                reach) while the lights show their own effects. */}
            <div
              className="picture-controls"
              data-quiet={mode === 'device_native'}
              inert={mode === 'device_native'}
            >
              {children}
            </div>
          </>
        ) : null}
      </section>
    </div>
  );
}
