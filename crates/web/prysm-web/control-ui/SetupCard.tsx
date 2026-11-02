import type { Config } from './api';

export function SetupCard({ config }: { config: Config }) {
  const { input, output } = config;
  return <section className="card setup-card" aria-labelledby="setup-heading">
    <div className="setup-heading">
      <p className="eyebrow">YOUR SETUP</p>
      <h2 id="setup-heading">Screen & lights</h2>
    </div>
    <div className="setup-flow">
      <div>
        <span className="setup-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
            <rect x="3" y="4" width="18" height="13" rx="2" />
            <path d="M8 21h8m-4-4v4" />
          </svg>
        </span>
        <strong>{input.type === 'webos' ? 'LG webOS' : input.type === 'device' ? 'Video capture' : 'Video file'}</strong>
        <span>{input.capture_resolution.join(' × ')} · {input.refresh_rate_hz} fps</span>
      </div>
      <span className="flow-arrow" aria-hidden="true">
        <svg viewBox="0 0 36 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <path d="M3 8h2m5 0h2m5 0h15m-5-5 5 5-5 5" />
        </svg>
      </span>
      <div>
        <span className="setup-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
            <path d="M9 17h6m-5 3h4M8 13a6 6 0 1 1 8 0c-1 1-1 2-1 2H9s0-1-1-2Z" />
          </svg>
        </span>
        <strong>WLED lights</strong>
        <span>Ambient color</span>
      </div>
    </div>
    <figure className="led-layout">
      <svg viewBox="0 0 360 224" role="img" aria-labelledby="led-layout-title">
        <title id="led-layout-title">LED layout: {output.leds.top} top, {output.leds.right} right, {output.leds.bottom} bottom, {output.leds.left} left</title>
        <defs>
          <linearGradient id="setup-led-color" x2="1" y2="1">
            <stop stopColor="#c080b4" />
            <stop offset=".5" stopColor="#9b86cf" />
            <stop offset="1" stopColor="#7cbbc9" />
          </linearGradient>
          <linearGradient id="setup-screen-glass" x2=".9" y2="1">
            <stop stopColor="#3d3b50" />
            <stop offset="1" stopColor="#272b3a" />
          </linearGradient>
        </defs>
        <rect className="screen-body" x="78" y="54" width="204" height="116" rx="5" />
        <path className="led-edge" d="M90 45h180 M291 66v92 M270 179H90 M69 158V66" />
        <path className="screen-stand" d="M180 171v20m-28 0h56" />
        <text x="180" y="27" textAnchor="middle">{output.leds.top}</text>
        <text x="325" y="116" textAnchor="middle">{output.leds.right}</text>
        <text x="180" y="216" textAnchor="middle">{output.leds.bottom}</text>
        <text x="35" y="116" textAnchor="middle">{output.leds.left}</text>
        <text className="screen-label" x="180" y="106" textAnchor="middle">{Object.values(output.leds).reduce((sum, count) => sum + count, 0)} LEDs</text>
        <text className="screen-sublabel" x="180" y="128" textAnchor="middle">around your screen</text>
      </svg>
    </figure>
    <details className="diagnostics setup-diagnostics">
      <summary>Connection details</summary>
      <dl className="facts">
        {input.type !== 'webos' ? <div>
          <dt>Video source</dt>
          <dd>{input.type === 'device' ? input.device : input.path}</dd>
        </div> : null}
        <div><dt>Light controller</dt><dd>{output.url}</dd></div>
      </dl>
    </details>
  </section>;
}
