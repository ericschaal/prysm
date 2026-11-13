import { useId, useState, type ReactNode } from 'react';
import {
  describeLighting,
  describeOutputIssue,
  failureOf,
  type Failure,
  type LightingTarget,
  type OutputIssue,
  type Session,
} from '@prysm/application-client';
import type { Input } from '@prysm/contracts/application';
import type { OutputLighting } from './adapters';

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
  /** Shown between the state and the controls: the live picture. */
  children?: ReactNode;
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

/** The state in plain words, for the headline. */
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

/** The light's glyph on the main switch: a screen throwing light around it. */
function SyncGlyph() {
  return (
    <svg
      className="power-glyph"
      viewBox="0 0 32 32"
      aria-hidden="true"
      focusable="false"
    >
      <rect x="8" y="10" width="16" height="11" rx="2" />
      <path d="M13 25h6M3.5 12v7M28.5 12v7M11 5.5h10" />
    </svg>
  );
}

/**
 * The lighting state as a headline, one main switch for screen sync and the
 * other modes as quiet choices. Each button sets a `LightingTarget`; the
 * state stream, not the click, decides what is shown as active.
 */
export function LightingControls({
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
  children,
}: LightingProps) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const [error, setError] = useState('');
  const headingId = useId();
  const statusId = useId();
  const effects = output?.effects ?? 'Device effects';
  const faulted = session.state === 'faulted';
  /** Screen sync is on, or starting; after a failure the switch retries. */
  const syncing =
    target === 'processed' && !(session.state === 'stopped' && failure);

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
  const syncReason = reasonOf(missingSetup(hasCapture, hasOutput));
  const alternatives: {
    value: LightingTarget;
    label: string;
    reason: string | null;
  }[] = [
    {
      value: 'device_native',
      label: effects,
      reason: reasonOf(missingEffects(hasOutput, output?.effects)),
    },
    { value: 'off', label: 'Off', reason: reasonOf(null) },
  ];
  const locked = disabled || pending !== null;

  return (
    <section
      className="lighting"
      data-session={session.state}
      data-lighting={target}
      aria-labelledby={headingId}
      aria-busy={pending !== null}
    >
      <h2 id={headingId} className="visually-hidden">
        Lighting
      </h2>
      <p
        id={statusId}
        className="lighting-status"
        role="status"
        aria-live="polite"
      >
        {headline({ target, session, failure, effects })}
      </p>
      {input && session.state === 'running' && !input.receiving ? (
        <p className="lighting-detail" role="status">
          Waiting for video.
        </p>
      ) : null}
      {outputIssue && session.state === 'running' ? (
        <p className="notice warning output-issue" role="status">
          {describeOutputIssue(outputIssue)}
        </p>
      ) : null}
      {failure ? (
        <p className="notice error" role="alert">
          {failure.message}
        </p>
      ) : null}
      {error && error !== failure?.message ? (
        <p className="notice error" role="alert">
          {error}
        </p>
      ) : null}
      {children}
      <div
        className="modes"
        role="group"
        aria-label="Lighting mode"
        aria-describedby={statusId}
      >
        <button
          type="button"
          className="power"
          data-target="processed"
          aria-pressed={syncing}
          aria-busy={pending === 'processed' || (pending === 'off' && syncing)}
          aria-describedby={syncReason ? `${headingId}-processed` : undefined}
          disabled={locked || syncReason !== null}
          onClick={() => void choose(syncing ? 'off' : 'processed')}
        >
          <span className="power-orb" aria-hidden="true">
            <SyncGlyph />
          </span>
          <span className="mode-label">Screen sync</span>
        </button>
        {syncReason ? (
          <small id={`${headingId}-processed`} className="mode-reason">
            {syncReason}
          </small>
        ) : null}
        <div className="mode-alternatives">
          {alternatives.map((mode) => (
            <button
              type="button"
              key={mode.value}
              className="mode-alternative"
              data-target={mode.value}
              aria-pressed={target === mode.value}
              aria-busy={pending === mode.value}
              aria-describedby={
                mode.reason ? `${headingId}-${mode.value}` : undefined
              }
              disabled={locked || mode.reason !== null}
              onClick={() => void choose(mode.value)}
            >
              {mode.label}
            </button>
          ))}
        </div>
        {alternatives.map((mode) =>
          mode.reason ? (
            <small
              key={mode.value}
              id={`${headingId}-${mode.value}`}
              className="visually-hidden"
            >
              {mode.reason}
            </small>
          ) : null,
        )}
      </div>
      {output?.link ? (
        <p className="device-link">
          <a href={output.link.href} target="_blank" rel="noopener noreferrer">
            {output.link.label}
            <span className="visually-hidden"> (opens in a new tab)</span>
          </a>
        </p>
      ) : null}
    </section>
  );
}
