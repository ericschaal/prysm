import type { Config, StateResponse, Target } from './api';

export function Overview({ config, data, busy, connectionError, requestError, requested, setState, poll }: {
  config: Config | null; data: StateResponse | null; busy: boolean; connectionError: string; requestError: string; requested: Target | null;
  setState: (target: Target) => Promise<void>; poll: () => Promise<void>;
}) {
  const available = !!data && !connectionError;
  const observed = available ? data.observed : null;
  const source = data?.source ?? 'prysm';
  const restart = !!data?.restart_required;
  const confirmed = !!observed && ((!observed.power || source === 'wled') ? !observed.realtime && observed.live_override === 2 && observed.pipeline_phase === 'idle' : observed.realtime && observed.live_override === 0 && observed.pipeline_phase === 'running');
  const mismatch = !!observed && !confirmed;
  const power = observed && (observed.power || confirmed) ? observed.power ? 'On' : 'Off' : 'Unknown';
  const disabled = !available || !observed || busy || restart;
  const operationError = data?.operation.kind === 'failed' ? data.operation.error.message : '';
  const retryTarget = requested || (observed ? { power: observed.power, source } : null);
  return <div className="overview-grid"><section className={`power-panel card ${power === 'On' ? 'is-on' : ''}`} aria-label="Light power" aria-busy={busy}>
    <p className="eyebrow">YOUR LIGHTS</p><div className="power-heading"><h2>{power}</h2><button id="power" className="power-button" disabled={disabled} aria-label={power === 'Unknown' ? 'Power unavailable' : observed?.power ? 'Turn lights off' : 'Turn lights on'} aria-pressed={power === 'On'} onClick={() => void setState({ power: !observed?.power, source })}><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M12 3v9M6.3 5.7a8 8 0 1 0 11.4 0" /></svg></button></div>
    <p id="status" role="status">{connectionError ? 'Connection lost. Checking again automatically.' : busy ? 'Applying the accepted change…' : restart ? 'Restart Prysm to apply the saved settings.' : mismatch ? 'Selected state is not confirmed. Check capture and WLED status.' : observed ? 'Power and source reflect the latest WLED observation.' : data ? 'Light status unknown. WLED may be unreachable.' : 'Connecting to your lights…'}</p>
    <fieldset className="sources" disabled={disabled}><legend>Color source</legend><div className="source-options">{([{ value: 'prysm', title: 'Screen sync', detail: 'Colors from the captured picture' }, { value: 'wled', title: 'WLED effects', detail: 'The effect selected on your lights' }] satisfies { value: Target['source']; title: string; detail: string }[]).map(option => <button type="button" data-source={option.value} key={option.value} className="source" aria-pressed={source === option.value} onClick={() => source !== option.value && void setState({ power: observed!.power, source: option.value })}><strong>{option.title}</strong><span>{option.detail}</span></button>)}</div></fieldset>
    {!observed || mismatch || requestError || operationError ? <div className="recovery"><button id="retry" disabled={busy || restart} onClick={() => available && retryTarget ? void setState(retryTarget) : void poll()}>{available && retryTarget ? 'Retry change' : 'Check connection'}</button>{available ? <button id="off" disabled={busy || restart} onClick={() => void setState({ power: false, source })}>Turn lights off</button> : null}</div> : null}
    {config?.output.type === 'wled' ? <a id="wled" className="wled-link" href={config.output.url} target="_blank" rel="noopener noreferrer">Customize effects in WLED ↗</a> : null}
  </section><div className="status-stack"><section className="card" aria-labelledby="operation-heading"><p className="eyebrow">APPLICATION</p><h2 id="operation-heading">What’s happening</h2><dl className="facts"><div><dt>Capture</dt><dd>{data?.pipeline.phase ?? 'Connecting'}</dd></div><div><dt>Operation</dt><dd>{data?.operation.kind ?? 'Unknown'}</dd></div><div><dt>Processed frames</dt><dd>{data?.pipeline.frames_processed ?? '—'}</dd></div><div><dt>Saved / applied revision</dt><dd>{data?.config_revision ?? '—'} / {data?.applied_revision ?? '—'}</dd></div></dl>
    {connectionError ? <p className="notice error" role="alert">Controller connection: {connectionError}</p> : null}
    {data?.observation_error ? <p className="notice warning">WLED observation: {data.observation_error}</p> : null}
    {data?.pipeline.error ? <p className="notice warning">Capture: {data.pipeline.error}</p> : null}
    {operationError ? <p className="notice error" role="alert">Operation failed: {operationError}</p> : null}
    {requestError ? <p className="notice error" role="alert">{requestError}</p> : null}
    {restart ? <p className="notice warning">Saved configuration requires an application restart. Power and source changes are unavailable until restart.</p> : null}
    {data?.pipeline.phase === 'completed' ? <p className="notice">Input completed. HTTP remains available; preview retains the final picture.</p> : null}
  </section>{config ? <section className="card" aria-labelledby="setup-heading"><p className="eyebrow">CURRENT SETUP</p><h2 id="setup-heading">Input & output</h2><dl className="facts"><div><dt>Input</dt><dd>{config.input.type}</dd></div><div><dt>Source</dt><dd>{config.input.type === 'device' ? config.input.device : config.input.type === 'file' ? config.input.path : config.input.listen}</dd></div><div><dt>Capture</dt><dd>{config.input.capture_resolution.join(' × ')} · {config.input.refresh_rate_hz} Hz</dd></div><div><dt>Output</dt><dd>{config.output.type === 'wled' ? config.output.url : config.output.type}</dd></div>{config.output.type === 'wled' ? <div><dt>LEDs · top / right / bottom / left</dt><dd>{[config.output.leds.top, config.output.leds.right, config.output.leds.bottom, config.output.leds.left].join(' / ')}</dd></div> : null}</dl><p className="caption">Input, output and service settings are read-only here. This interface edits settings that apply live.</p></section> : null}</div></div>;
}
