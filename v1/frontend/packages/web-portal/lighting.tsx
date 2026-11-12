import { useId, useState } from 'react';
import type {
  Failure,
  LightingTarget,
  Session,
} from '@prysm/application-client';
import { failureOf, type OutputLighting } from './adapters';

const phases = {
  running: 'running',
  closing_source: 'closing the capture source',
  idling_output: 'idling the lights',
  finishing: 'finishing',
} as const;

export interface LightingProps {
  target: LightingTarget;
  session: Session;
  failure: Failure | null;
  hasCapture: boolean;
  hasOutput: boolean;
  /** What the configured output offers, `null` without an output. */
  output: OutputLighting | null;
  disabled: boolean;
  setTarget: (target: LightingTarget) => Promise<unknown>;
}

/** What the lights are doing, in words, from the requested target and the session. */
export function describeLighting({
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
      return `Prysm stopped responding while ${phases[session.phase]}. Restart Prysm to recover.`;
    case 'running':
      return 'Screen sync is running.';
    case 'native':
      return `${effects} are on.`;
    case 'stopped':
      if (target === 'off') return 'Lights are off.';
      if (failure)
        return target === 'processed'
          ? 'Screen sync stopped.'
          : `${effects} could not start.`;
      return target === 'processed' ? 'Starting screen sync…' : `Starting ${effects}…`;
  }
}

/**
 * The three lighting modes. Each button sets a `LightingTarget`; the state
 * stream, not the click, decides what is shown as active.
 */
export function LightingControls({
  target,
  session,
  failure,
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
      unavailable:
        !hasCapture && !hasOutput
          ? 'Configure capture and lights first.'
          : !hasCapture
            ? 'Configure capture first.'
            : !hasOutput
              ? 'Configure your lights first.'
              : null,
    },
    {
      value: 'device_native',
      label: effects,
      hint: 'The lights run their own effects.',
      unavailable: !hasOutput
        ? 'Configure your lights first.'
        : !output?.effects
          ? 'These lights have no effects of their own.'
          : null,
    },
    { value: 'off', label: 'Off', hint: 'Turn the lights off.', unavailable: null },
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
        <span
          className={`state state-${session.state}`}
          aria-hidden="true"
        />
      </div>
      <p id={statusId} className="lighting-status" role="status" aria-live="polite">
        {describeLighting({ target, session, failure, effects })}
      </p>
      <div className="modes" role="group" aria-label="Lighting mode" aria-describedby={statusId}>
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
