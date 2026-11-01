import { useCallback, useEffect, useRef, useState, type SVGProps } from 'react';
import { client, unwrap, type StateResponse, type Config, type Target } from './api';
import './style.css';
import { createRoot } from 'react-dom/client';

function confirmed(observed: NonNullable<StateResponse['observed']>, source: Target['source'], pipelinePower: boolean) {
  if (!observed.power || source === 'wled') {
    return !observed.realtime && observed.live_override === 2 && observed.pipeline_phase === 'idle';
  }
  return observed.realtime && observed.live_override === 0 && (observed.pipeline_phase === 'running' || (pipelinePower && observed.pipeline_phase === 'completed'));
}

function Icon({ name, ...props }: SVGProps<SVGSVGElement> & { name: string }) {
  return <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...props}>
    {name === 'power' ? <><path d="M12 3v8" /><path d="M6.3 5.7a8 8 0 1 0 11.4 0" /></> :
      name === 'screen' ? <><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8M12 17v4" /></> :
      name === 'arrow' ? <><path d="M7 17 17 7M7 7h10v10" /></> :
      <><path d="m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5Z" /></>}
  </svg>;
}

function ShadowLift({ config, appliedRevision, disabled, active, onSave }: { config: Config; appliedRevision: string | null | undefined; disabled: boolean; active: boolean; onSave: (value: number) => Promise<void> }) {
  const [draft, setDraft] = useState<number | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const saved = config.processing.shadow_lift_percent;
  const value = draft ?? saved;

  useEffect(() => {
    if (draft === null || saving || disabled || error) return;
    const timer = setTimeout(async () => {
      setSaving(true);
      try {
        await onSave(draft);
        setDraft(null);
      } catch {
        setError('Save not confirmed. Retry to save this value.');
      } finally {
        setSaving(false);
      }
    }, 300);
    return () => clearTimeout(timer);
  }, [draft, saving, disabled, error, onSave]);

  const applied = appliedRevision === config.revision;
  const message = error || (saving ? 'Saving…' : draft !== null ? 'Unsaved change' :
    active && applied ? 'Applied to screen sync' : active ? 'Saved · waiting for the processor' : 'Saved · applies when screen sync starts');
  return <section className="shadow-lift" aria-labelledby="shadow-label" aria-busy={saving}>
    <div className="setting-heading"><label id="shadow-label" htmlFor="shadow-lift">Shadow lift</label>
      <output htmlFor="shadow-lift">{value === 0 ? 'Off' : `${value}%`}</output></div>
    <p id="shadow-help">Bring out dark-scene colors while keeping black off.</p>
    <input id="shadow-lift" type="range" min="0" max="100" step="1" value={value}
      disabled={disabled || saving} aria-describedby="shadow-help shadow-status"
      aria-valuetext={value === 0 ? 'Off' : `${value} percent`}
      onChange={event => { const next = Number(event.target.value); setDraft(next === saved ? null : next); setError(''); }} />
    <div className="setting-status"><span id="shadow-status" role="status" aria-live="polite">{message}</span>
      {error ? <button disabled={disabled || saving} onClick={() => setError('')}>Retry save</button> : null}</div>
  </section>;
}

function App() {
  const [data, setData] = useState<StateResponse | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [busy, setBusy] = useState(false);
  const [awaiting, setAwaiting] = useState<string | null>(null);
  const [connectionError, setConnectionError] = useState('');
  const [requestError, setRequestError] = useState('');
  const operation = useRef<{ busy: boolean; polling: boolean; revision: number; awaiting: string | null; requested: Target | null; action: 'source' | 'power' | null }>({ busy: false, polling: false, revision: 0, awaiting: null, requested: null, action: null });

  const poll = useCallback(async () => {
    const current = operation.current;
    if (document.hidden || current.busy || current.polling) return;
    current.polling = true;
    const revision = current.revision;
    try {
      let snapshot: { state: StateResponse; config: Config } | undefined;
      for (let attempt = 0; attempt < 2; attempt += 1) {
        const [stateResult, configResult] = await Promise.all([client.GET('/api/state'), client.GET('/api/config')]);
        const state = unwrap(stateResult);
        const config = unwrap(configResult);
        if (state.config_revision === config.revision) { snapshot = { state, config }; break; }
      }
      if (!snapshot) throw Error('Configuration changed while loading. Checking again automatically.');
      const next = snapshot.state;
      if (revision !== current.revision) return;
      setData(next);
      setConfig(snapshot.config);
      setConnectionError('');
      // A fresh observation may already include a later operation from another client.
      if (current.awaiting !== null && next.operation.kind !== 'pending') {
        current.awaiting = null;
        setAwaiting(null);
        setRequestError(next.operation.kind === 'idle' ? 'Prysm restarted; completion is unknown. Check status or retry.' : '');
        if (next.operation.kind === 'succeeded') current.requested = null;
      }
      if (next.operation.kind !== 'pending' && current.awaiting === null) current.action = null;
    } catch (error) {
      if (revision === current.revision) setConnectionError((error instanceof Error ? error.message : 'Request failed.'));
    } finally {
      current.polling = false;
    }
  }, []);

  useEffect(() => {
    poll();
    const timer = setInterval(poll, 2000);
    document.addEventListener('visibilitychange', poll);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', poll);
      operation.current.revision += 1;
    };
  }, [poll]);

  async function setState(target: Target) {
    const current = operation.current;
    if (current.busy || data?.operation.kind === 'pending' || current.awaiting !== null) return;
    current.busy = true;
    current.revision += 1;
    current.requested = target;
    current.action = target.power === data?.observed?.power ? 'source' : 'power';
    setBusy(true);
    setRequestError('');
    try {
      const accepted = unwrap(await client.PUT('/api/state', { body: target }));
      current.awaiting = accepted.operation_id;
      setAwaiting(accepted.operation_id);
    } catch (error) {
      setRequestError(`${(error instanceof Error ? error.message : 'Request failed.')} The change may have been accepted. Check status before retrying.`);
    } finally {
      current.busy = false;
      setBusy(false);
      await poll();
    }
  }

  const saveSettings = useCallback(async (value: number) => {
    if (!config) return;
    operation.current.revision += 1;
    const saved = unwrap(await client.PUT('/api/config', { body: {
      expected_revision: config.revision,
      config: { ...config, processing: { ...config.processing, shadow_lift_percent: value } },
    } }));
    operation.current.revision += 1;
    setConfig(saved);
  }, [config]);

  const wledControl = config?.output.type === 'wled';
  const directOutput = !!config && !wledControl;
  const observed = connectionError ? null : data?.observed;
  const pending = busy || data?.operation.kind === 'pending' || awaiting !== null;
  const mismatch = !!observed && !confirmed(observed, data!.source, !wledControl);
  const available = !!data && !connectionError;
  const restartRequired = !!data?.restart_required;
  const operationError = data?.operation.kind === 'failed' ? data.operation.error.message : '';
  const error = connectionError || requestError || data?.observation_error || operationError || data?.pipeline.error;
  const needsAttention = !!error || mismatch || restartRequired;
  const power = observed?.power ? 'On' : observed && !mismatch ? 'Off' : 'Unknown';
  const source = data?.source || 'prysm';
  const powerPending = pending && operation.current.action === 'power';
  const sourcePending = pending && operation.current.action === 'source' ? operation.current.requested?.source : null;
  let message = data ? '' : 'Connecting to your lights…';
  if (connectionError) message = 'Connection lost. Checking again automatically.';
  else if (pending) message = '';
  else if (restartRequired) message = 'Restart Prysm to apply the saved settings.';
  else if (requestError) message = 'Change not confirmed. Check status or retry.';
  else if (data?.observation_error || (data && !observed)) message = 'Light status unknown. Check device connections and capture.';
  else if (operationError) message = 'Couldn’t apply the change. Try again.';
  else if (data?.pipeline.phase === 'starting') message = 'Capture is starting. Screen sync will resume when ready.';
  else if (directOutput && data?.pipeline.phase === 'completed') message = 'Playback complete. Turn off and on to replay.';
  else if (mismatch) message = 'Selected state is not confirmed. Retry or turn the lights off.';

  const retryTarget = operation.current.requested || (observed ? { power: observed.power, source } : null);
  const disabled = !available || !observed || pending || restartRequired;

  return <main>
    <header>
      <a className="brand" href="/" aria-label="Prysm home">
        <svg className="logo" viewBox="0 0 32 32" fill="none" aria-hidden="true"><path d="m16 3 13 24H3L16 3Z" stroke="currentColor" strokeWidth="2" /><path d="m16 3 4 24M3 27l15-12 11 12" stroke="currentColor" strokeWidth="1.5" /></svg>
        <span>prysm<span className="brand-dot">.</span></span>
      </a>
      <span className={`connection ${pending ? 'is-pending' : ''} ${connectionError || needsAttention ? 'attention' : available ? 'ready' : ''}`}><i />{connectionError ? 'Offline' : needsAttention ? 'Check status' : available ? 'Connected' : 'Connecting'}</span>
    </header>

    <div className="intro"><h1>Set the mood.</h1></div>

    <section className={`power-panel ${power === 'On' ? 'is-on' : ''}`} aria-label={directOutput ? 'Output power' : 'Light power'} aria-busy={powerPending}>
      <div className="power-heading">
        <div className="power-copy"><p className="eyebrow">{directOutput ? 'OUTPUT' : 'LIGHTS'}</p>
          <h2 aria-live="polite"><span key={power}>{power}</span></h2>
        </div>
        <button id="power" className="power-button" disabled={disabled}
          aria-label={power === 'Unknown' ? 'Power unavailable' : observed?.power ? (directOutput ? 'Stop output' : 'Turn lights off') : (directOutput ? 'Start output' : 'Turn lights on')}
          aria-pressed={power === 'On'}
          onClick={() => setState({ power: !observed?.power, source })}><Icon name="power" /></button>
      </div>
      <div className="light-spectrum" aria-hidden="true"><span /><span /><span /><span /><span /></div>
      <div id="status" className={`status ${message ? 'visible' : ''} ${needsAttention ? 'warning' : ''}`} role="status" aria-live="polite">{message ? <><i />{message}</> : null}</div>
    </section>

    {needsAttention ? <section className="recovery" aria-label="Connection and recovery">
      <div className="recovery-actions">
        <button id="retry" disabled={pending || restartRequired} onClick={() => available && retryTarget ? setState(retryTarget) : poll()}>{available && retryTarget ? 'Retry change' : 'Check connection'}</button>
        {available ? <button id="off" disabled={pending || restartRequired} onClick={() => setState({ power: false, source })}>{directOutput ? 'Stop output' : 'Turn lights off'}</button> : null}
      </div>
      {error ? <details><summary>Technical details</summary><p>{error}</p></details> : null}
    </section> : null}

    {power === 'On' && wledControl ? <fieldset disabled={disabled} className="sources" aria-busy={pending && !powerPending}>
      <legend>Color source</legend>
      <div className="source-options">{([
        { value: 'prysm', icon: 'screen', title: 'Screen sync' },
        { value: 'wled', icon: 'spark', title: 'WLED effects' },
      ] satisfies { value: Target['source']; icon: string; title: string }[]).map(option => <button key={option.value} data-source={option.value} className="source" aria-pressed={source === option.value} aria-busy={sourcePending === option.value}
        onClick={() => source !== option.value && setState({ power: observed!.power, source: option.value })}>
        <span className="source-icon"><Icon name={option.icon} /></span>
        <span className="source-title">{option.title}</span>
        <span className="selection" aria-hidden="true"><svg viewBox="0 0 16 16" fill="none"><path d="m4 8 2.5 2.5L12 5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" /></svg></span>
      </button>)}</div>
    </fieldset> : null}

    {source === 'prysm' && config ? <ShadowLift config={config} appliedRevision={data?.applied_revision}
      disabled={!available || pending} active={!!observed?.power && !!observed?.realtime && !mismatch}
      onSave={saveSettings} /> : null}

    {data?.wled_url && wledControl ? <a id="wled" className="wled-link" href={data.wled_url} target="_blank" rel="noopener noreferrer"><span>Customize in WLED</span><Icon name="arrow" /></a> : null}
  </main>;
}

createRoot(document.getElementById('root')!).render(<App />);
