import type { SettingsValues } from './api';
import { LedLayout } from './LedLayout';

const inputTitles = {
  webos: 'LG webOS',
  device: 'Video capture',
  file: 'Video file',
};

export function SetupCard({
  input,
  output,
}: {
  input: NonNullable<SettingsValues['input']>;
  output: NonNullable<SettingsValues['output']>;
}) {
  return (
    <section className="card setup-card" aria-labelledby="setup-heading">
      <div className="setup-heading">
        <p className="eyebrow">YOUR SETUP</p>
        <h2 id="setup-heading">Screen & lights</h2>
      </div>
      <div className="setup-flow">
        <div>
          <span className="setup-icon" aria-hidden="true">
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
            >
              <rect x="3" y="4" width="18" height="13" rx="2" />
              <path d="M8 21h8m-4-4v4" />
            </svg>
          </span>
          <strong>{inputTitles[input.type]}</strong>
          <span>
            {input.capture_resolution.join(' × ')} · {input.refresh_rate_hz} fps
          </span>
        </div>
        <span className="flow-arrow" aria-hidden="true">
          <svg
            viewBox="0 0 36 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M3 8h2m5 0h2m5 0h15m-5-5 5 5-5 5" />
          </svg>
        </span>
        <div>
          <span className="setup-icon" aria-hidden="true">
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
            >
              <path d="M9 17h6m-5 3h4M8 13a6 6 0 1 1 8 0c-1 1-1 2-1 2H9s0-1-1-2Z" />
            </svg>
          </span>
          <strong>WLED lights</strong>
          <span>Ambient color</span>
        </div>
      </div>
      <LedLayout leds={output.leds} />
      <details className="diagnostics setup-diagnostics">
        <summary>Connection details</summary>
        <dl className="facts">
          {input.type !== 'webos' ? (
            <div>
              <dt>Video source</dt>
              <dd>{input.type === 'device' ? input.device : input.path}</dd>
            </div>
          ) : null}
          <div>
            <dt>Light controller</dt>
            <dd>{output.url}</dd>
          </div>
        </dl>
      </details>
    </section>
  );
}
