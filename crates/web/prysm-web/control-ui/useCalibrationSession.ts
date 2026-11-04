import { useEffect, useEffectEvent, useRef, useState } from 'react';
import { ApiError, client, unwrap } from './api';
import type { components } from './api.generated';
type View = components['schemas']['CalibrationView'];
type Request = components['schemas']['CalibrationRequest'];
type Action =
  | { action: 'select'; profile: string | null }
  | { action: 'save'; name: string }
  | { action: 'cancel' };
type DraftPatch = Partial<
  Pick<
    Extract<Request, { action: 'update' }>,
    'correction' | 'step' | 'shade' | 'paused'
  >
>;

export function useCalibrationSession(pairing: string) {
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState('');
  const [connectionError, setConnectionError] = useState('');
  const rejectedRef = useRef(false);
  const [busy, setBusy] = useState(false);
  const currentRef = useRef<View | null>(null);
  const heldRef = useRef(false);
  const sequenceRef = useRef(0);
  const pendingRef = useRef<Extract<Request, { action: 'update' }> | null>(
    null,
  );
  const sendingRef = useRef(false);
  const aliveRef = useRef(true);
  const key = `prysm-calibration-${pairing}`;

  function receive(next: View) {
    if (!aliveRef.current) return;
    if (next.sequence < sequenceRef.current && !next.ended) return;
    sequenceRef.current = Math.max(sequenceRef.current, next.sequence);
    currentRef.current = next;
    setView(next);
    if (next.ended) {
      heldRef.current = false;
      pendingRef.current = null;
      sessionStorage.removeItem(key);
    }
  }
  function report(failure: unknown) {
    if (aliveRef.current)
      setError(
        failure instanceof Error
          ? failure.message
          : 'Connection lost. Reconnecting…',
      );
  }
  async function request(body: Request) {
    return unwrap(await client.POST('/api/calibration', { body }));
  }
  async function flush() {
    if (sendingRef.current) return;
    sendingRef.current = true;
    try {
      while (pendingRef.current && aliveRef.current) {
        const next = pendingRef.current;
        receive(await request(next));
        if (pendingRef.current === next) pendingRef.current = null;
        if (aliveRef.current) setError('');
      }
    } catch (failure) {
      report(failure);
    } finally {
      sendingRef.current = false;
    }
  }
  function update(patch: DraftPatch, compare = heldRef.current) {
    const previous = currentRef.current;
    if (!previous || previous.ended || rejectedRef.current) return;
    const next = {
      ...previous,
      step: patch.step ?? previous.step,
      shade: patch.shade ?? previous.shade,
      paused: patch.paused ?? previous.paused,
      draft: patch.correction
        ? { ...previous.draft, correction: patch.correction }
        : previous.draft,
      sequence: ++sequenceRef.current,
      compare,
    };
    currentRef.current = next;
    setView(next);
    pendingRef.current = {
      action: 'update',
      token: next.token,
      sequence: next.sequence,
      correction: next.draft.correction,
      step: next.step,
      shade: next.shade,
      paused: next.paused,
      compare,
    };
    void flush();
  }
  function comparison(value: boolean) {
    if (heldRef.current === value) return;
    heldRef.current = value;
    update({}, value);
  }
  async function action(body: Action) {
    const active = currentRef.current;
    if (!active || active.ended || rejectedRef.current) return;
    if (
      sendingRef.current ||
      (pendingRef.current && body.action !== 'cancel')
    ) {
      setError('Wait for the current adjustment to apply, then try again.');
      return;
    }
    sendingRef.current = true;
    setBusy(true);
    setError('');
    try {
      const next = await request({ ...body, token: active.token });
      receive(next);
    } catch (failure) {
      report(failure);
    } finally {
      sendingRef.current = false;
      if (aliveRef.current) setBusy(false);
      void flush();
    }
  }
  const receiveEvent = useEffectEvent(receive);
  const comparisonEvent = useEffectEvent(comparison);
  const flushEvent = useEffectEvent(flush);
  useEffect(() => {
    aliveRef.current = true;
    let timer: ReturnType<typeof setTimeout>;
    const stored = sessionStorage.getItem(key);
    let disposed = false;
    async function poll() {
      const active = currentRef.current;
      if (disposed || rejectedRef.current || active?.ended) return;
      if (!sendingRef.current) {
        sendingRef.current = true;
        try {
          const token = active?.token ?? stored;
          const next = await request(
            token
              ? {
                  action: 'poll',
                  token,
                  playing: false,
                  compare: heldRef.current,
                }
              : { action: 'pair', pairing },
          );
          if (disposed) return;
          if (!active) {
            sessionStorage.setItem(key, next.token);
            sequenceRef.current = next.sequence;
          }
          receiveEvent(next);
          setConnectionError('');
        } catch (failure) {
          if (disposed) return;
          if (
            failure instanceof ApiError &&
            failure.status >= 400 &&
            failure.status < 500 &&
            ![408, 429].includes(failure.status)
          ) {
            rejectedRef.current = true;
            pendingRef.current = null;
            sessionStorage.removeItem(key);
          }
          setConnectionError(
            failure instanceof Error
              ? failure.message
              : 'Connection lost. Reconnecting…',
          );
        } finally {
          sendingRef.current = false;
          if (!disposed && !rejectedRef.current) void flushEvent();
        }
      }
      if (!disposed && !rejectedRef.current)
        timer = setTimeout(() => void poll(), 1000);
    }
    void poll();
    const release = () => comparisonEvent(false);
    const hidden = () => {
      if (document.hidden) release();
    };
    window.addEventListener('blur', release);
    window.addEventListener('pagehide', release);
    document.addEventListener('visibilitychange', hidden);
    return () => {
      release();
      aliveRef.current = false;
      disposed = true;
      clearTimeout(timer);
      window.removeEventListener('blur', release);
      window.removeEventListener('pagehide', release);
      document.removeEventListener('visibilitychange', hidden);
    };
    // Pairing owns the lifetime; callbacks read the current draft through refs.
  }, [pairing, key]);
  async function stop() {
    heldRef.current = false;
    pendingRef.current = null;
    try {
      unwrap(
        await client.PUT('/api/state', {
          body: { power: false, source: 'prysm' },
        }),
      );
    } catch (failure) {
      report(failure);
    }
  }
  return {
    view,
    error: error || connectionError,
    busy,
    update,
    comparison,
    selectProfile: (profile: string | null) =>
      action({ action: 'select', profile }),
    save: (name: string) => action({ action: 'save', name }),
    cancel: () => action({ action: 'cancel' }),
    stop,
  };
}
