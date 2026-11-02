import { useCallback, useEffect, useRef, useState } from 'react';
import { client, unwrap, type StateResponse, type Config, type Target } from './api';

export function useControl() {
  const [data, setData] = useState<StateResponse | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [busy, setBusy] = useState(false);
  const [pendingControl, setPendingControl] = useState<'power' | Target['source'] | null>(null);
  const [awaiting, setAwaiting] = useState<string | null>(null);
  const [connectionError, setConnectionError] = useState('');
  const [requestError, setRequestError] = useState('');
  const operation = useRef({ busy: false, polling: false, revision: 0, awaiting: null as string | null });
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
      if (!snapshot) throw Error('Settings changed while loading. Checking again automatically.');
      if (revision !== current.revision) return;
      setData(snapshot.state);
      setConfig(snapshot.config);
      setConnectionError('');
      if (current.awaiting !== null && snapshot.state.operation.kind !== 'pending') {
        current.awaiting = null;
        setAwaiting(null);
        setPendingControl(null);
        setRequestError(snapshot.state.operation.kind === 'idle' ? 'Prysm restarted; completion is unknown. Check status before retrying.' : '');
      }
    } catch (error) {
      if (revision === current.revision) setConnectionError(error instanceof Error ? error.message : 'Request failed.');
    } finally { current.polling = false; }
  }, []);
  useEffect(() => {
    void poll();
    const timer = setInterval(poll, 2000);
    document.addEventListener('visibilitychange', poll);
    return () => { clearInterval(timer); document.removeEventListener('visibilitychange', poll); operation.current.revision += 1; };
  }, [poll]);

  async function setState(target: Target, control: 'power' | 'source') {
    const current = operation.current;
    if (current.busy || data?.operation.kind === 'pending' || current.awaiting !== null) return;
    current.busy = true;
    current.revision += 1;
    setBusy(true); setPendingControl(control === 'power' ? 'power' : target.source); setRequestError('');
    try {
      const accepted = unwrap(await client.PUT('/api/state', { body: target }));
      current.awaiting = accepted.operation_id; setAwaiting(accepted.operation_id);
    } catch (error) {
      setPendingControl(null);
      setRequestError(`${error instanceof Error ? error.message : 'Request failed.'} The change may have been accepted. Check status before retrying.`);
    } finally { current.busy = false; setBusy(false); void poll(); }
  }

  async function saveConfig(next: Config) {
    const current = operation.current;
    if (current.busy || data?.operation.kind === 'pending' || current.awaiting !== null) throw Error('A change is already running.');
    current.busy = true; current.revision += 1; setBusy(true);
    try {
      const saved = unwrap(await client.PUT('/api/config', { body: { expected_revision: next.revision, config: next } }));
      current.revision += 1; setConfig(saved);
      current.awaiting = `config:${saved.revision}`; setAwaiting(current.awaiting);
      return saved;
    } finally { current.busy = false; setBusy(false); void poll(); }
  }
  return { data, config, pendingControl, busy: busy || awaiting !== null || data?.operation.kind === 'pending', connectionError, requestError, poll, setState, saveConfig };
}
