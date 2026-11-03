import { useEffect, useRef } from 'react';
import type { SettingsView, StateResponse, Target } from './api';
import { SetupCard } from './SetupCard';

export function Overview({
  settings,
  data,
  busy,
  pendingControl,
  connectionError,
  requestError,
  setState,
}: {
  settings: SettingsView | null;
  data: StateResponse | null;
  busy: boolean;
  pendingControl: 'power' | Target['source'] | null;
  connectionError: string;
  requestError: string;
  setState: (target: Target, control: 'power' | 'source') => Promise<void>;
}) {
  const values = settings?.settings;
  const available = !!data && !connectionError;
  const observed = available ? data.observed : null;
  const powered = observed?.power;
  const previousPowerRef = useRef(powered);
  const powerPanelRef = useRef<HTMLElement>(null);
  const powerButtonRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const turnedOn = previousPowerRef.current === false && powered === true;
    previousPowerRef.current = powered;
    if (!turnedOn) return;
    if (powerPanelRef.current && powerButtonRef.current) {
      const panel = powerPanelRef.current.getBoundingClientRect();
      const button = powerButtonRef.current.getBoundingClientRect();
      powerPanelRef.current.style.setProperty(
        '--light-x',
        `${button.x + button.width / 2 - panel.x}px`,
      );
      powerPanelRef.current.style.setProperty(
        '--light-y',
        `${button.y + button.height / 2 - panel.y}px`,
      );
    }
    const panel = powerPanelRef.current;
    panel?.classList.add('is-blooming');
    const timer = setTimeout(() => panel?.classList.remove('is-blooming'), 700);
    return () => {
      clearTimeout(timer);
      panel?.classList.remove('is-blooming');
    };
  }, [powered]);
  const source = data?.source ?? 'prysm';
  const confirmed =
    !!observed &&
    (!observed.power || source === 'wled'
      ? !observed.realtime &&
        observed.live_override === 2 &&
        observed.pipeline_phase === 'idle'
      : observed.realtime &&
        observed.live_override === 0 &&
        observed.pipeline_phase === 'running');
  const mismatch = !!observed && !confirmed;
  const power = observed ? (observed.power ? 'On' : 'Off') : 'Unknown';
  const turnOn = observed ? !observed.power : false;
  const disabled = !available || busy;
  const notice = connectionError
    ? 'Connection lost. Reconnecting automatically…'
    : !observed
      ? data
        ? 'Light status unavailable. The power button can still send an off command.'
        : 'Connecting to your lights…'
      : !busy && mismatch && observed.power
        ? `Select ${source === 'prysm' ? 'Screen sync' : 'WLED effects'} to reconnect.`
        : '';
  const operationError =
    data?.operation.kind === 'failed' ? data.operation.error.message : '';
  return (
    <div className="overview-grid">
      <section
        ref={powerPanelRef}
        className={`power-panel card ${power === 'On' ? 'is-on' : ''}`}
        aria-label="Light power"
        aria-busy={busy}
      >
        <div className="power-identity">
          <p className="eyebrow">YOUR LIGHTS</p>
          <div className="power-heading">
            <h2 role="status" aria-live="polite">
              {power}
            </h2>
            <button
              ref={powerButtonRef}
              id="power"
              className="power-button"
              disabled={disabled}
              aria-busy={pendingControl === 'power'}
              aria-label={turnOn ? 'Turn lights on' : 'Turn lights off'}
              title={turnOn ? 'Turn lights on' : 'Turn lights off'}
              aria-describedby={notice ? 'status' : undefined}
              aria-pressed={observed ? observed.power : undefined}
              onClick={() => void setState({ power: turnOn, source }, 'power')}
            >
              <svg
                viewBox="0 0 24 24"
                aria-hidden="true"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.8"
              >
                <path d="M12 3v9M6.3 5.7a8 8 0 1 0 11.4 0" />
              </svg>
            </button>
          </div>
          {notice ? (
            <p id="status" role="status">
              {notice}
            </p>
          ) : null}
        </div>
        <div className="power-controls">
          <fieldset className="sources" disabled={disabled || !observed}>
            <legend>Color source</legend>
            <div className="source-options" data-selected={source}>
              {(
                [
                  { value: 'prysm', title: 'Screen sync' },
                  { value: 'wled', title: 'WLED effects' },
                ] satisfies { value: Target['source']; title: string }[]
              ).map((option) => (
                <button
                  type="button"
                  data-source={option.value}
                  key={option.value}
                  className="source"
                  aria-busy={pendingControl === option.value}
                  aria-pressed={source === option.value}
                  onClick={() =>
                    (source !== option.value ||
                      mismatch ||
                      !!requestError ||
                      !!operationError) &&
                    void setState(
                      { power: observed!.power, source: option.value },
                      'source',
                    )
                  }
                >
                  <svg
                    className="source-icon"
                    viewBox="0 0 64 48"
                    aria-hidden="true"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.8"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  >
                    {option.value === 'prysm' ? (
                      <>
                        <rect
                          className="source-aura"
                          x="8"
                          y="6"
                          width="48"
                          height="31"
                          rx="6"
                        />
                        <defs>
                          <linearGradient id="source-picture" x2="1" y2="1">
                            <stop stopColor="#dba6d9" />
                            <stop offset=".5" stopColor="#b5a1ef" />
                            <stop offset="1" stopColor="#8bd4e3" />
                          </linearGradient>
                        </defs>
                        <rect
                          x="13"
                          y="11"
                          width="38"
                          height="23"
                          rx="3"
                          fill="url(#source-picture)"
                          fillOpacity=".24"
                        />
                        <path d="M26 41h12m-6-7v7" />
                        <path
                          className="source-accent"
                          d="M18 7h28M9 16v12m46-12v12"
                        />
                      </>
                    ) : (
                      <>
                        <path
                          className="source-aura"
                          d="M11 33c11-28 27 12 42-18"
                        />
                        <path d="M11 24c11-28 27 12 42-18M11 42c11-28 27 12 42-18" />
                        <path
                          className="source-accent"
                          d="M11 33c11-28 27 12 42-18"
                        />
                      </>
                    )}
                  </svg>
                  <strong>{option.title}</strong>
                </button>
              ))}
            </div>
          </fieldset>
          <div className="source-link">
            {values?.output?.type === 'wled' && source === 'wled' ? (
              <a
                id="wled"
                className="wled-link"
                href={values.output.url}
                target="_blank"
                rel="noopener noreferrer"
              >
                Open WLED ↗
              </a>
            ) : null}
          </div>
        </div>
        {connectionError ? (
          <p className="notice error" role="alert">
            Controller connection: {connectionError}
          </p>
        ) : null}
        {data?.observation_error ? (
          <p className="notice warning">
            WLED observation: {data.observation_error}
          </p>
        ) : null}
        {data?.pipeline.error ? (
          <p className="notice warning">Capture: {data.pipeline.error}</p>
        ) : null}
        {operationError ? (
          <p className="notice error" role="alert">
            Operation failed: {operationError}
          </p>
        ) : null}
        {requestError ? (
          <p className="notice error" role="alert">
            {requestError}
          </p>
        ) : null}
        <details className="diagnostics power-diagnostics">
          <summary>Technical details</summary>
          <dl className="facts">
            <div>
              <dt>Capture</dt>
              <dd>
                {data
                  ? {
                      idle: 'Idle',
                      starting: 'Starting',
                      running: 'Capturing',
                      stopping: 'Stopping',
                      failed: 'Failed',
                      completed: 'Finished',
                    }[data.pipeline.phase]
                  : 'Connecting'}
              </dd>
            </div>
            <div>
              <dt>Last change</dt>
              <dd>
                {data
                  ? {
                      idle: 'None yet',
                      pending: 'Applying',
                      succeeded: 'Applied',
                      failed: 'Failed',
                    }[data.operation.kind]
                  : '—'}
              </dd>
            </div>
            <div>
              <dt>Frames processed</dt>
              <dd>{data?.pipeline.frames_processed.toLocaleString() ?? '—'}</dd>
            </div>
            <div>
              <dt>Settings revision</dt>
              <dd className="revision-values">
                <span>
                  {data?.config_revision ?? '—'} <small>saved</small>
                </span>
                <span>
                  {data?.applied_revision ?? '—'} <small>applied</small>
                </span>
              </dd>
            </div>
          </dl>
        </details>
      </section>
      {values?.input && values.output ? (
        <SetupCard input={values.input} output={values.output} />
      ) : null}
    </div>
  );
}
