import { useEffect, useRef, useState } from 'react';
import type { Config, StateResponse, Target } from './api';

export function Overview({ config, data, busy, pendingControl, connectionError, requestError, setState }: {
  config: Config | null; data: StateResponse | null; busy: boolean; pendingControl: 'power' | Target['source'] | null; connectionError: string; requestError: string;
  setState: (target: Target, control: 'power' | 'source') => Promise<void>;
}) {
  const available = !!data && !connectionError;
  const observed = available ? data.observed : null;
  const powered = observed?.power;
  const previousPower = useRef(powered);
  const powerPanel = useRef<HTMLElement>(null);
  const powerButton = useRef<HTMLButtonElement>(null);
  const [blooming, setBlooming] = useState(false);
  useEffect(() => {
    const turnedOn = previousPower.current === false && powered === true;
    previousPower.current = powered;
    if (!turnedOn) { if (powered !== true) setBlooming(false); return; }
    if (powerPanel.current && powerButton.current) {
      const panel = powerPanel.current.getBoundingClientRect();
      const button = powerButton.current.getBoundingClientRect();
      powerPanel.current.style.setProperty('--light-x', `${button.x + button.width / 2 - panel.x}px`);
      powerPanel.current.style.setProperty('--light-y', `${button.y + button.height / 2 - panel.y}px`);
    }
    setBlooming(true);
    const timer = setTimeout(() => setBlooming(false), 700);
    return () => clearTimeout(timer);
  }, [powered]);
  const source = data?.source ?? 'prysm';
  const restart = !!data?.restart_required;
  const confirmed = !!observed && ((!observed.power || source === 'wled') ? !observed.realtime && observed.live_override === 2 && observed.pipeline_phase === 'idle' : observed.realtime && observed.live_override === 0 && observed.pipeline_phase === 'running');
  const mismatch = !!observed && !confirmed;
  const power = observed ? observed.power ? 'On' : 'Off' : 'Unknown';
  const turnOn = observed ? !observed.power : false;
  const disabled = !available || busy || restart;
  const notice = connectionError ? 'Connection lost. Reconnecting automatically…' : restart ? 'Restart Prysm to apply the saved settings.' : !observed ? data ? 'Light status unavailable. The power button can still send an off command.' : 'Connecting to your lights…' : !busy && mismatch && observed.power ? `Select ${source === 'prysm' ? 'Screen sync' : 'WLED effects'} to reconnect.` : '';
  const operationError = data?.operation.kind === 'failed' ? data.operation.error.message : '';
  return <div className="overview-grid"><section ref={powerPanel} className={`power-panel card ${power === 'On' ? 'is-on' : ''} ${blooming ? 'is-blooming' : ''}`} aria-label="Light power" aria-busy={busy}>
    <div className="power-identity"><p className="eyebrow">YOUR LIGHTS</p><div className="power-heading"><h2 role="status" aria-live="polite">{power}</h2><button ref={powerButton} id="power" className="power-button" disabled={disabled} aria-busy={pendingControl === 'power'} aria-label={turnOn ? 'Turn lights on' : 'Turn lights off'} title={turnOn ? 'Turn lights on' : 'Turn lights off'} aria-describedby={notice ? 'status' : undefined} aria-pressed={observed ? observed.power : undefined} onClick={() => void setState({ power: turnOn, source }, 'power')}><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M12 3v9M6.3 5.7a8 8 0 1 0 11.4 0" /></svg></button></div>
    {notice ? <p id="status" role="status">{notice}</p> : null}
    </div><div className="power-controls"><fieldset className="sources" disabled={disabled || !observed}><legend>Color source</legend><div className="source-options" data-selected={source}>{([{ value: 'prysm', title: 'Screen sync' }, { value: 'wled', title: 'WLED effects' }] satisfies { value: Target['source']; title: string }[]).map(option => <button type="button" data-source={option.value} key={option.value} className="source" aria-busy={pendingControl === option.value} aria-pressed={source === option.value} onClick={() => (source !== option.value || mismatch || !!requestError || !!operationError) && void setState({ power: observed!.power, source: option.value }, 'source')}>
      <svg className="source-icon" viewBox="0 0 64 48" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">{option.value === 'prysm' ? <><rect className="source-aura" x="8" y="6" width="48" height="31" rx="6" /><rect x="13" y="11" width="38" height="23" rx="3" /><path d="M26 41h12m-6-7v7" /><path className="source-accent" d="m18 28 9-9 9 6 10-9" /></> : <><path className="source-aura" d="M11 33c11-28 27 12 42-18" /><path d="M11 24c11-28 27 12 42-18M11 42c11-28 27 12 42-18" /><path className="source-accent" d="M11 33c11-28 27 12 42-18" /></>}</svg>
      <strong>{option.title}</strong></button>)}</div></fieldset>
    <div className="source-link">{config?.output.type === 'wled' && source === 'wled' ? <a id="wled" className="wled-link" href={config.output.url} target="_blank" rel="noopener noreferrer">Open WLED ↗</a> : null}</div>
    </div>
    {connectionError ? <p className="notice error" role="alert">Controller connection: {connectionError}</p> : null}
    {data?.observation_error ? <p className="notice warning">WLED observation: {data.observation_error}</p> : null}
    {data?.pipeline.error ? <p className="notice warning">Capture: {data.pipeline.error}</p> : null}
    {operationError ? <p className="notice error" role="alert">Operation failed: {operationError}</p> : null}
    {requestError ? <p className="notice error" role="alert">{requestError}</p> : null}
    {restart ? <p className="notice warning">Saved configuration requires an application restart. Power and source changes are unavailable until restart.</p> : null}
    <details className="diagnostics power-diagnostics"><summary>Technical details</summary><dl className="facts">
      <div><dt>Capture</dt><dd>{data ? ({ idle: 'Idle', starting: 'Starting', running: 'Capturing', stopping: 'Stopping', failed: 'Failed', completed: 'Finished' }[data.pipeline.phase]) : 'Connecting'}</dd></div>
      <div><dt>Last change</dt><dd>{data ? ({ idle: 'None yet', pending: 'Applying', succeeded: 'Applied', failed: 'Failed' }[data.operation.kind]) : '—'}</dd></div>
      <div><dt>Frames processed</dt><dd>{data?.pipeline.frames_processed.toLocaleString() ?? '—'}</dd></div>
      <div><dt>Settings revision</dt><dd className="revision-values"><span>{data?.config_revision ?? '—'} <small>saved</small></span><span>{data?.applied_revision ?? '—'} <small>applied</small></span></dd></div>
    </dl></details>
  </section>{config ? <section className="card setup-card" aria-labelledby="setup-heading"><div className="setup-heading"><p className="eyebrow">YOUR SETUP</p><h2 id="setup-heading">Screen & lights</h2></div>
    <div className="setup-flow"><div><span className="setup-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8m-4-4v4" /></svg></span><strong>{config.input.type === 'webos' ? 'LG webOS' : config.input.type === 'device' ? 'Video capture' : 'Video file'}</strong><span>{config.input.capture_resolution.join(' × ')} · {config.input.refresh_rate_hz} fps</span></div><span className="flow-arrow" aria-hidden="true">→</span><div><span className="setup-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M9 17h6m-5 3h4M8 13a6 6 0 1 1 8 0c-1 1-1 2-1 2H9s0-1-1-2Z" /></svg></span><strong>{config.output.type === 'wled' ? 'WLED lights' : 'Preview output'}</strong><span>Ambient color</span></div></div>
    {config.output.type === 'wled' ? <figure className="led-layout"><svg viewBox="0 0 360 224" role="img" aria-labelledby="led-layout-title"><title id="led-layout-title">LED layout: {config.output.leds.top} top, {config.output.leds.right} right, {config.output.leds.bottom} bottom, {config.output.leds.left} left</title><rect className="screen-halo" x="66" y="42" width="228" height="140" rx="20" /><rect className="screen-body" x="78" y="54" width="204" height="116" rx="12" /><path className="led-edge" d="M90 45h180 M291 66v92 M270 179H90 M69 158V66" /><path className="screen-stand" d="M180 171v20m-28 0h56" /><text x="180" y="27" textAnchor="middle">{config.output.leds.top}</text><text x="325" y="116" textAnchor="middle">{config.output.leds.right}</text><text x="180" y="216" textAnchor="middle">{config.output.leds.bottom}</text><text x="35" y="116" textAnchor="middle">{config.output.leds.left}</text><text className="screen-label" x="180" y="106" textAnchor="middle">{Object.values(config.output.leds).reduce((sum, count) => sum + count, 0)} LEDs</text><text className="screen-sublabel" x="180" y="128" textAnchor="middle">around your screen</text></svg></figure> : null}

    <details className="diagnostics"><summary>Connection details</summary><dl className="facts"><div><dt>Video source</dt><dd>{config.input.type === 'device' ? config.input.device : config.input.type === 'file' ? config.input.path : config.input.listen}</dd></div><div><dt>Light controller</dt><dd>{config.output.type === 'wled' ? config.output.url : config.output.type}</dd></div></dl></details></section> : null}</div>;
}
