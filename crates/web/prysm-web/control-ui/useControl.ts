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
type Control = 'power' | Target['source'] | 'settings';
type PendingOperation = { kind: 'lighting'; id: string } | { kind: 'settings' };

function completionError(
  pending: PendingOperation,
  result: StateResponse['operation'] | undefined,
) {
  if (pending.kind === 'settings') return '';
  if (!result || result.kind === 'idle' || result.id !== pending.id)
    return 'Prysm restarted; completion is unknown. Check status before retrying.';
  return result.kind === 'failed' ? result.error.message : '';
}

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
  const [activeControl, setActiveControl] = useState<Control | null>(null);
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
        setRequestError(
          completionError(current.pending, snapshot.state?.operation),
        );
        current.pending = null;
        setActiveControl(null);
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

  function beginWrite(control: Control) {
    const current = operation.current;
    if (
      current.writing ||
      current.pending ||
      data?.operation.kind === 'pending'
    )
      return false;
    current.writing = true;
    current.generation += 1;
    setActiveControl(control);
    return true;
  }
  function finishWrite() {
    operation.current.writing = false;
    if (!operation.current.pending) setActiveControl(null);
    void poll();
  }
  async function setState(target: Target, control: 'power' | 'source') {
    if (!beginWrite(control === 'power' ? 'power' : target.source)) return;
    setRequestError('');
    try {
      const response = unwrap(await client.PUT('/api/state', { body: target }));
      operation.current.pending = {
        kind: 'lighting',
        id: response.operation_id,
      };
    } catch (error) {
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
    if (!beginWrite('settings')) throw Error('A change is already running.');
    try {
      const saved = unwrap(
        await client.PUT('/api/settings', {
          body: { expected_revision, settings: changes },
        }),
      );
      operation.current.generation += 1;
      setSettings(saved);
      operation.current.pending = { kind: 'settings' };
      return saved;
    } finally {
      finishWrite();
    }
  }
  return {
    data,
    settings,
    pendingControl: activeControl === 'settings' ? null : activeControl,
    busy: activeControl !== null || data?.operation.kind === 'pending',
    connectionError,
    requestError,
    poll,
    setState,
    saveSettings,
  };
}
