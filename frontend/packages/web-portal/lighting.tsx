import {
  useEffect,
  useId,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { useLingering } from './lingering';
import { arrowChoice } from './radio-group';
import {
  failureOf,
  same,
  type Failure,
  type LightingSituation,
  type LightingTarget,
  type TargetReading,
} from '@prysm/application-client';
import {
  lightCardView,
  moodOf,
  pauseOf,
  words,
  type LightCardFacts,
  type Pause,
} from './light-card-view';

export interface LightingProps extends LightCardFacts {
  disabled: boolean;
  setTarget: (target: LightingTarget) => Promise<unknown>;
  /** Opens the guided setup; offered while capture or lights are missing. */
  openSetup: () => void;
  /** The screen sync picture controls, under the mode tiles. */
  children?: ReactNode;
}

/** Starting is mentioned only once it takes longer than this. */
export const slowStartMs = 1000;

/**
 * The power button's busy ring: shown only once an operation has taken this
 * long (a quick one shows none), and then kept at least `ringHoldMs`, so it
 * never flickers.
 */
export const ringDelayMs = 350;
export const ringHoldMs = 450;

function PowerIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M12 3.5v8M6.6 6.2a7.6 7.6 0 1 0 10.8 0" />
    </svg>
  );
}

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
 * Your lights: the state as one word and the power button, then the
 * controls (what the lights follow, then the picture's brightness, style
 * and calibration), over a light that drifts while the lights are on. Each control sets a `LightingTarget`; the state
 * stream, not the click, decides what is shown as on.
 */
export function LightCard({
  disabled,
  setTarget,
  openSetup,
  children,
  ...facts
}: LightingProps) {
  const { target, control, lighting, hasCapture, hasOutput, output } = facts;
  const [pending, setPending] = useState<LightingTarget | null>(null);
  /**
   * The latest refused click, with the lights as they were then: a command
   * that timed out may still have run, so it is shown only until they change.
   */
  const [refusal, setRefusal] = useState<{
    failure: Failure;
    /** What the click asked for. */
    asked: LightingTarget;
    target: TargetReading;
    kind: LightingSituation['kind'];
  } | null>(null);
  // Gone once the lights change, also when they change back.
  if (refusal && (refusal.target !== target || refusal.kind !== lighting.kind))
    setRefusal(null);
  const headingId = useId();
  const statusId = useId();

  async function choose(next: LightingTarget) {
    const at = { target, kind: lighting.kind };
    setPending(next);
    setRefusal(null);
    try {
      await setTarget(next);
    } catch (reason) {
      setRefusal({ failure: failureOf(reason), asked: next, ...at });
    } finally {
      setPending(null);
    }
  }

  const configured = hasCapture && hasOutput;
  const mood = moodOf(lighting, configured);
  // Switching modes passes through starting: the light stays up meanwhile.
  const running = mood === 'sync' || mood === 'effects';
  const [illumination, setIllumination] = useState<{
    lit: boolean;
    ignite?: 'a' | 'b';
  }>({ lit: running });
  const lit = running || (mood === 'starting' && illumination.lit);
  let { ignite } = illumination;
  if (lit !== illumination.lit) {
    if (lit) ignite = ignite === 'a' ? 'b' : 'a';
    setIllumination({ lit, ignite });
  }

  const pause = pauseOf(lighting, lit);
  const [lasting, setLasting] = useState<Pause | null>(null);
  useEffect(() => {
    if (!pause) return;
    const timer = setTimeout(() => setLasting(pause), slowStartMs);
    return () => {
      clearTimeout(timer);
      setLasting(null);
    };
  }, [pause]);
  const note = lasting === pause ? pause : null;
  const {
    effects,
    on,
    reasons,
    mode,
    chosen,
    powerReason,
    word,
    palette,
    status,
    info,
    calm,
    ownEffects,
  } = lightCardView(facts, { error: refusal, lit, note });
  const locked = disabled || pending !== null;
  // Busy: a click on its way, or the lights starting (not yet lit).
  const ring = useLingering(
    pending !== null || (mood === 'starting' && !lit),
    ringDelayMs,
    ringHoldMs,
  );

  const [kept, setKept] = useState(info);
  if (info && !same(info, kept)) setKept(info);
  const shown = info ?? kept;

  const tiles = [
    {
      value: 'screen_sync',
      label: 'Screen sync',
      line: 'Follows the screen',
      icon: <SyncIcon />,
    },
    {
      value: 'effects',
      label: effects,
      line: effectsLine(output?.effects),
      icon: <EffectsIcon />,
    },
  ] as const;
  /** Arrow keys move the choice, as in any radio group. */
  function arrow(event: KeyboardEvent<HTMLButtonElement>) {
    arrowChoice(
      event,
      tiles.map(({ value }) => value),
      // From the focused tile: the chosen one, or the first while none is.
      chosen ?? tiles[0].value,
      (next) => {
        if (locked || reasons[next]) return false;
        void choose(next);
        return true;
      },
    );
  }
  // One visible line per distinct reason; every tile keeps its own for AT.
  const shownReasons = new Set<string>();

  return (
    <div
      className="light-frame"
      data-lit={lit}
      data-ignite={ignite}
      data-palette={palette}
    >
      <section
        className="light-card"
        data-mood={mood}
        data-mode={chosen ?? undefined}
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
                aria-busy={
                  control?.busy ||
                  (pending !== null && (pending === 'off') === on)
                }
                data-ring={ring}
                aria-describedby={
                  powerReason
                    ? `${headingId}-power`
                    : `${statusId} ${headingId}-starts`
                }
                disabled={locked || control?.busy || powerReason !== null}
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
              : `Turns on ${mode === 'screen_sync' ? 'Screen sync' : effects}.`}
          </small>
        )}
        {configured ? (
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
                <div className="light-info-actions">
                  {shown.retry ? (
                    <button
                      type="button"
                      disabled={locked}
                      onClick={() => {
                        if (lighting.kind === 'failed')
                          void choose(lighting.target);
                      }}
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
        {configured && on ? (
          <>
            {/* The mode: two option cards, each saying what it does. */}
            {output?.effects && (
              <div
                className="modes"
                role="radiogroup"
                aria-label="Mode"
                data-mode={chosen ?? undefined}
                data-on={on}
              >
                {tiles.map((tile) => (
                  <button
                    type="button"
                    role="radio"
                    key={tile.value}
                    className="mode"
                    data-target={tile.value}
                    aria-checked={chosen === tile.value}
                    // With none chosen, the first one takes the focus.
                    tabIndex={
                      (chosen ?? tiles[0].value) === tile.value ? 0 : -1
                    }
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
                      if (target !== tile.value || lighting.kind === 'failed')
                        void choose(tile.value);
                    }}
                  >
                    {tile.icon}
                    <span className="mode-name">{tile.label}</span>
                    <span className="mode-line">{tile.line}</span>
                  </button>
                ))}
              </div>
            )}
            {output?.effects &&
              tiles.map(({ value }) => {
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
              data-quiet={chosen === 'effects'}
              inert={chosen !== 'screen_sync'}
            >
              {children}
            </div>
          </>
        ) : null}
      </section>
    </div>
  );
}
