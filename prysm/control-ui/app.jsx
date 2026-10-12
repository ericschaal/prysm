import { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';

async function api(method = 'GET', body) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 5000);
  try {
    const response = await fetch('/api/state', {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: body && JSON.stringify(body),
      signal: controller.signal,
    });
    if (!response.ok) throw Error(`Request failed (${response.status}).`);
    return await response.json();
  } finally {
    clearTimeout(timer);
  }
}

function confirmed(observed, source) {
  if (!observed.power || source === 'wled') {
    return !observed.realtime && observed.live_override === 2 && observed.service === 'inactive';
  }
  return observed.realtime && observed.live_override === 0 && observed.service === 'active';
}

function Icon({ name, ...props }) {
  return <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...props}>
    {name === 'power' ? <><path d="M12 3v8" /><path d="M6.3 5.7a8 8 0 1 0 11.4 0" /></> :
      name === 'screen' ? <><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8M12 17v4" /></> :
      name === 'arrow' ? <><path d="M7 17 17 7M7 7h10v10" /></> :
      <><path d="m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5Z" /></>}
  </svg>;
}

function App() {
  const [data, setData] = useState(null);
  const [busy, setBusy] = useState(false);
  const [awaiting, setAwaiting] = useState(null);
  const [connectionError, setConnectionError] = useState('');
  const [requestError, setRequestError] = useState('');
  const operation = useRef({ busy: false, polling: false, revision: 0, awaiting: null, requested: null, action: null });

  const poll = useCallback(async () => {
    const current = operation.current;
    if (document.hidden || current.busy || current.polling) return;
    current.polling = true;
    const revision = current.revision;
    try {
      const next = await api();
      if (revision !== current.revision) return;
      setData(next);
      setConnectionError('');
      if (current.awaiting !== null && !next.pending && next.operation_id >= current.awaiting) {
        current.awaiting = null;
        setAwaiting(null);
        setRequestError('');
        if (!next.error) current.requested = null;
      }
      if (!next.pending && current.awaiting === null) current.action = null;
    } catch (error) {
      if (revision === current.revision) setConnectionError(error.message);
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

  async function setState(target) {
    const current = operation.current;
    if (current.busy || data?.pending || current.awaiting !== null) return;
    current.busy = true;
    current.revision += 1;
    current.requested = target;
    current.action = target.power === data?.observed?.power ? 'source' : 'power';
    setBusy(true);
    setRequestError('');
    try {
      const accepted = await api('PUT', target);
      current.awaiting = accepted.operation_id;
      setAwaiting(accepted.operation_id);
    } catch (error) {
      setRequestError(`${error.message} The change may have been accepted. Check status before retrying.`);
    } finally {
      current.busy = false;
      setBusy(false);
      await poll();
    }
  }

  const observed = connectionError ? null : data?.observed;
  const pending = busy || !!data?.pending || awaiting !== null;
  const mismatch = !!observed && !confirmed(observed, data.source);
  const available = !!data && !connectionError;
  const error = connectionError || requestError || data?.observation_error || data?.error;
  const needsAttention = !!error || mismatch;
  const power = observed?.power ? 'On' : observed && !mismatch ? 'Off' : 'Unknown';
  const source = data?.source || 'prysm';
  const powerPending = pending && operation.current.action === 'power';
  const sourcePending = pending && operation.current.action === 'source' ? operation.current.requested?.source : null;
  let message = data ? '' : 'Connecting to your lights…';
  if (connectionError) message = 'Connection lost. Checking again automatically.';
  else if (pending) message = '';
  else if (requestError) message = 'Change not confirmed. Check status or retry.';
  else if (data?.observation_error || (data && !observed)) message = 'Light status unknown. Check the WLED connection.';
  else if (data?.error) message = 'Couldn’t apply the change. Try again.';
  else if (observed?.substate === 'auto-restart') message = 'Prysm is restarting. Screen sync will resume when ready.';
  else if (mismatch) message = 'Selected state is not confirmed. Retry or turn the lights off.';

  const retryTarget = operation.current.requested || (observed ? { power: observed.power, source } : null);
  const disabled = !available || !observed || pending;

  return <main>
    <header>
      <a className="brand" href="/" aria-label="Prysm home">
        <svg className="logo" viewBox="0 0 32 32" fill="none" aria-hidden="true"><path d="m16 3 13 24H3L16 3Z" stroke="currentColor" strokeWidth="2" /><path d="m16 3 4 24M3 27l15-12 11 12" stroke="currentColor" strokeWidth="1.5" /></svg>
        <span>prysm<span className="brand-dot">.</span></span>
      </a>
      <span className={`connection ${pending ? 'is-pending' : ''} ${connectionError || needsAttention ? 'attention' : available ? 'ready' : ''}`}><i />{connectionError ? 'Offline' : needsAttention ? 'Check status' : available ? 'Connected' : 'Connecting'}</span>
    </header>

    <div className="intro"><h1>Set the mood.</h1></div>

    <section className={`power-panel ${power === 'On' ? 'is-on' : ''}`} aria-label="Light power" aria-busy={powerPending}>
      <div className="power-heading">
        <div className="power-copy"><p className="eyebrow">LIGHTS</p>
          <h2 aria-live="polite"><span key={power}>{power}</span></h2>
        </div>
        <button id="power" className="power-button" disabled={disabled}
          aria-label={power === 'Unknown' ? 'Power unavailable' : observed.power ? 'Turn lights off' : 'Turn lights on'}
          aria-pressed={power === 'On'}
          onClick={() => setState({ power: !observed.power, source })}><Icon name="power" /></button>
      </div>
      <div className="light-spectrum" aria-hidden="true"><span /><span /><span /><span /><span /></div>
      <div id="status" className={`status ${message ? 'visible' : ''} ${needsAttention ? 'warning' : ''}`} role="status" aria-live="polite">{message ? <><i />{message}</> : null}</div>
    </section>

    {needsAttention ? <section className="recovery" aria-label="Connection and recovery">
      <div className="recovery-actions">
        <button id="retry" disabled={pending} onClick={() => available && retryTarget ? setState(retryTarget) : poll()}>{available && retryTarget ? 'Retry change' : 'Check connection'}</button>
        {available ? <button id="off" disabled={pending} onClick={() => setState({ power: false, source })}>Turn lights off</button> : null}
      </div>
      {error ? <details><summary>Technical details</summary><p>{error}</p></details> : null}
    </section> : null}

    {power === 'On' ? <fieldset disabled={disabled} className="sources" aria-busy={pending && !powerPending}>
      <legend>Color source</legend>
      <div className="source-options">{[
        { value: 'prysm', icon: 'screen', title: 'Screen sync' },
        { value: 'wled', icon: 'spark', title: 'WLED effects' },
      ].map(option => <button key={option.value} data-source={option.value} className="source" aria-pressed={source === option.value} aria-busy={sourcePending === option.value}
        onClick={() => source !== option.value && setState({ power: observed.power, source: option.value })}>
        <span className="source-icon"><Icon name={option.icon} /></span>
        <span className="source-title">{option.title}</span>
        <span className="selection" aria-hidden="true"><svg viewBox="0 0 16 16" fill="none"><path d="m4 8 2.5 2.5L12 5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" /></svg></span>
      </button>)}</div>
    </fieldset> : null}

    {data?.wled_url ? <a id="wled" className="wled-link" href={data.wled_url} target="_blank" rel="noopener noreferrer"><span>Customize in WLED</span><Icon name="arrow" /></a> : null}
  </main>;
}

createRoot(document.getElementById('root')).render(<App />);
