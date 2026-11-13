import { useId, useState } from 'react';
import {
  describeLighting,
  describeOutputIssue,
  failureOf,
  type Failure,
  type LightingTarget,
  type OutputIssue,
  type Session,
} from '@prysm/application-client';
import type { OutputLighting } from './adapters';

export interface LightingProps {
  target: LightingTarget;
  session: Session;
  failure: Failure | null;
  /** Why the lights are not showing screen sync while it runs. */
  outputIssue?: OutputIssue;
  hasCapture: boolean;
  hasOutput: boolean;
  /** What the configured output offers, `null` without an output. */
  output: OutputLighting | null;
  disabled: boolean;
  setTarget: (target: LightingTarget) => Promise<unknown>;
}

/** Why screen sync cannot start yet, or `null`. */
function missingSetup(hasCapture: boolean, hasOutput: boolean) {
  if (!hasCapture && !hasOutput) return 'Configure capture and lights first.';
  if (!hasCapture) return 'Configure capture first.';
  if (!hasOutput) return 'Configure your lights first.';
  return null;
}

/** Why the output's own effects cannot start, or `null`. */
function missingEffects(hasOutput: boolean, effects: string | undefined) {
  if (!hasOutput) return 'Configure your lights first.';
  if (!effects) return 'These lights have no effects of their own.';
  return null;
}

/**
 * The three lighting modes. Each button sets a `LightingTarget`; the state
 * stream, not the click, decides what is shown as active.
 */
export function LightingControls({
  target,
  session,
  failure,
  outputIssue,
  hasCapture,
  hasOutput,
  output,
  disabled,
  setTarget,
}: LightingProps) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const [error, setError] = useState('');
  const headingId = useId();
  const statusId = useId();
  const effects = output?.effects ?? 'Device effects';
  const faulted = session.state === 'faulted';

  const modes: {
    value: LightingTarget;
    label: string;
    hint: string;
    unavailable: string | null;
  }[] = [
    {
      value: 'processed',
      label: 'Screen sync',
      hint: 'Colours follow the picture.',
      unavailable: missingSetup(hasCapture, hasOutput),
    },
    {
      value: 'device_native',
      label: effects,
      hint: 'The lights run their own effects.',
      unavailable: missingEffects(hasOutput, output?.effects),
    },
    {
      value: 'off',
      label: 'Off',
      hint: 'Turn the lights off.',
      unavailable: null,
    },
  ];

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

  return (
    <section
      className="card lighting"
      aria-labelledby={headingId}
      aria-busy={pending !== null}
    >
      <div className="card-heading">
        <h2 id={headingId}>Lighting</h2>
        <span className={`state state-${session.state}`} aria-hidden="true" />
      </div>
      <p
        id={statusId}
        className="lighting-status"
        role="status"
        aria-live="polite"
      >
        {describeLighting({ target, session, failure, effects })}
      </p>
      <div
        className="modes"
        role="group"
        aria-label="Lighting mode"
        aria-describedby={statusId}
      >
        {modes.map((mode) => {
          const reason = faulted
            ? 'Restart Prysm to change the lights.'
            : mode.unavailable;
          const hintId = `${headingId}-${mode.value}`;
          return (
            <div className="mode" key={mode.value}>
              <button
                type="button"
                data-target={mode.value}
                aria-pressed={target === mode.value}
                aria-busy={pending === mode.value}
                aria-describedby={hintId}
                disabled={disabled || pending !== null || reason !== null}
                onClick={() => void choose(mode.value)}
              >
                {mode.label}
              </button>
              <small id={hintId}>{reason ?? mode.hint}</small>
            </div>
          );
        })}
      </div>
      {output?.link ? (
        <p className="device-link">
          <a href={output.link.href} target="_blank" rel="noopener noreferrer">
            {output.link.label}
            <span className="visually-hidden"> (opens in a new tab)</span>
          </a>
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
    </section>
  );
}
