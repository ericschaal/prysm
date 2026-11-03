import { useCallback, useEffect, useRef, useState } from 'react';
import {
  client,
  unwrap,
  type StateResponse,
  type Target,
  type SettingsView,
  type SettingsValues,
} from './api';

type Snapshot = { state: StateResponse | null; settings: SettingsView };
type PendingOperation = { kind: 'lighting'; id: string } | { kind: 'settings' };

async function readSnapshot(): Promise<Snapshot> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const settings = unwrap(await client.GET('/api/settings'));
    if (settings.missing_required.length) return { state: null, settings };
    const state = unwrap(await client.GET('/api/state'));
    if (state.config_revision === settings.revision) return { state, settings };
  }
  throw Error('Settings changed while loading. Checking again automatically.');
}

export function useControl() {
  const [data, setData] = useState<StateResponse | null>(null);
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [writing, setWriting] = useState(false);
  const [pendingControl, setPendingControl] = useState<
    'power' | Target['source'] | null
  >(null);
  const [pending, setPending] = useState<PendingOperation | null>(null);
  const [connectionError, setConnectionError] = useState('');
  const [requestError, setRequestError] = useState('');
  const operation = useRef({
    writing: false,
    polling: false,
    generation: 0,
    pending: null as PendingOperation | null,
  });
  const poll = useCallback(async () => {
    const current = operation.current;
    if (document.hidden || current.writing || current.polling) return;
    current.polling = true;
    const generation = current.generation;
    try {
      const snapshot = await readSnapshot();
      if (generation !== current.generation) return;
      setData(snapshot.state);
      setSettings(snapshot.settings);
      setConnectionError('');
      if (current.pending && snapshot.state?.operation.kind !== 'pending') {
        const completed = snapshot.state?.operation;
        const unknown =
          current.pending.kind === 'lighting' &&
          (!completed ||
            completed.kind === 'idle' ||
            completed.id !== current.pending.id);
        current.pending = null;
        setPending(null);
        setPendingControl(null);
        setRequestError(
          unknown
            ? 'Prysm restarted; completion is unknown. Check status before retrying.'
            : '',
        );
      }
    } catch (error) {
      if (generation === current.generation)
        setConnectionError(
          error instanceof Error ? error.message : 'Request failed.',
        );
    } finally {
      current.polling = false;
    }
  }, []);
  useEffect(() => {
    const current = operation.current;
    const refresh = () => {
      void poll();
    };
    refresh();
    const timer = setInterval(refresh, 2000);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', refresh);
      current.generation += 1;
    };
  }, [poll]);

  function beginWrite() {
    const current = operation.current;
    if (
      current.writing ||
      current.pending ||
      data?.operation.kind === 'pending'
    )
      return false;
    current.writing = true;
    current.generation += 1;
    setWriting(true);
    return true;
  }
  function accepted(pending: PendingOperation) {
    operation.current.pending = pending;
    setPending(pending);
  }
  function finishWrite() {
    operation.current.writing = false;
    setWriting(false);
    void poll();
  }
  async function setState(target: Target, control: 'power' | 'source') {
    if (!beginWrite()) return;
    setPendingControl(control === 'power' ? 'power' : target.source);
    setRequestError('');
    try {
      const response = unwrap(await client.PUT('/api/state', { body: target }));
      accepted({ kind: 'lighting', id: response.operation_id });
    } catch (error) {
      setPendingControl(null);
      setRequestError(
        `${error instanceof Error ? error.message : 'Request failed.'} The change may have been accepted. Check status before retrying.`,
      );
    } finally {
      finishWrite();
    }
  }
  async function saveSettings(
    expected_revision: string,
    changes: SettingsValues,
  ) {
    if (!beginWrite()) throw Error('A change is already running.');
    try {
      const saved = unwrap(
        await client.PUT('/api/settings', {
          body: { expected_revision, settings: changes },
        }),
      );
      operation.current.generation += 1;
      setSettings(saved);
      accepted({ kind: 'settings' });
      return saved;
    } finally {
      finishWrite();
    }
  }
  return {
    data,
    settings,
    pendingControl,
    busy: writing || pending !== null || data?.operation.kind === 'pending',
    connectionError,
    requestError,
    poll,
    setState,
    saveSettings,
  };
}
