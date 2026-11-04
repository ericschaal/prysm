import { useEffect, useState } from 'react';
import { ApiError, client, unwrap } from './api';
import type { components } from './api.generated';

type View = components['schemas']['CalibrationView'];
type Request = components['schemas']['CalibrationRequest'];
type Update = Extract<Request, { action: 'update' }>;
type Heartbeat = Extract<Request, { action: 'pair' | 'poll' }>;
type Action = Extract<Request, { action: 'select' | 'save' | 'cancel' }>;
type DraftPatch = Partial<
  Pick<Update, 'correction' | 'step' | 'shade' | 'paused'>
>;

const initial = {
  view: null as View | null,
  error: '',
  rejected: false,
  busy: false,
  saving: false,
  canCancel: false,
};

function createPhoneSession(
  pairing: string,
  render: (state: typeof initial) => void,
) {
  const key = `prysm-calibration-${pairing}`;
  let view: View | null = null;
  let draft: Update | null = null;
  let action: Action | null = null;
  let sending: Request | null = null;
  let held = false;
  let mounted = false;
  let rejected = false;
  let error = '';
  let connectionError = '';

  function canCancel() {
    return !action || (action.action === 'save' && sending !== action);
  }
  function publish() {
    if (mounted)
      render({
        view,
        rejected,
        error: error || connectionError,
        busy: !!action,
        saving: action?.action === 'save',
        canCancel: !!view && !view.ended && canCancel(),
      });
  }

  function nextRequest(heartbeat?: Heartbeat) {
    if (heartbeat) return heartbeat;
    if (action?.action === 'cancel') return action;
    return draft ?? action;
  }

  // A heartbeat also wakes a failed draft. Save waits for its acknowledgment;
  // Cancel can discard it until the Save request is actually on the wire.
  async function send(heartbeat?: Heartbeat) {
    if (!mounted || sending || rejected || view?.ended) return;
    const body = nextRequest(heartbeat);
    if (!body) return;
    sending = body;
    if (body === action) publish();
    let continueSending = true;
    try {
      const next = unwrap(await client.POST('/api/calibration', { body }));
      if (!mounted) return;
      if (!view && !next.ended) sessionStorage.setItem(key, next.token);
      if (next.ended || !view || next.sequence >= view.sequence) view = next;
      if (draft === body) draft = null;
      if (heartbeat) connectionError = '';
      else error = '';
    } catch (failure) {
      if (!mounted) return;
      const message =
        failure instanceof Error
          ? failure.message
          : 'Connection lost. Reconnecting…';
      if (heartbeat) {
        connectionError = message;
        if (
          failure instanceof ApiError &&
          failure.status >= 400 &&
          failure.status < 500 &&
          ![408, 429].includes(failure.status)
        ) {
          rejected = true;
          view = null;
          error = '';
        }
      } else {
        error = message;
        continueSending = false;
      }
    } finally {
      sending = null;
      if (body === action) action = null;
    }
    if (view?.ended || rejected) {
      sessionStorage.removeItem(key);
      draft = null;
      action = null;
      held = false;
    }
    publish();
    if (continueSending || action?.action === 'cancel') void send();
  }

  function update({ correction, ...patch }: DraftPatch) {
    if (!view || view.ended) return;
    view = {
      ...view,
      ...patch,
      draft: correction ? { ...view.draft, correction } : view.draft,
      compare: held,
      sequence: view.sequence + 1,
    };
    const { token, sequence, step, shade, paused, compare } = view;
    draft = {
      action: 'update',
      token,
      sequence,
      step,
      shade,
      paused,
      compare,
      correction: view.draft.correction,
    };
    publish();
    void send();
  }
  function comparison(value: boolean) {
    if (held === value) return;
    held = value;
    update({});
  }
  function queue(command: Action) {
    if (!view || view.ended) return;
    if (action && command.action !== 'cancel') return;
    if (command.action === 'cancel' && !canCancel()) return;
    action = command;
    error = '';
    publish();
    void send();
  }
  function connect() {
    mounted = true;
    function poll() {
      if (rejected || view?.ended) {
        clearInterval(timer);
        return;
      }
      const token = view?.token ?? sessionStorage.getItem(key);
      void send(
        token
          ? { action: 'poll', token, playing: false, compare: held }
          : { action: 'pair', pairing },
      );
    }
    const release = () => comparison(false);
    const hidden = () => {
      if (document.hidden) release();
    };
    window.addEventListener('blur', release);
    window.addEventListener('pagehide', release);
    document.addEventListener('visibilitychange', hidden);
    const timer = setInterval(poll, 1000);
    poll();
    return () => {
      release();
      mounted = false;
      clearInterval(timer);
      window.removeEventListener('blur', release);
      window.removeEventListener('pagehide', release);
      document.removeEventListener('visibilitychange', hidden);
    };
  }
  return {
    connect,
    update,
    comparison,
    selectProfile: (profile: string | null) => {
      if (view) queue({ action: 'select', token: view.token, profile });
    },
    save: (name: string) => {
      if (view) queue({ action: 'save', token: view.token, name });
    },
    cancel: () => {
      if (view) queue({ action: 'cancel', token: view.token });
    },
  };
}

export function useCalibrationSession(pairing: string) {
  const [state, render] = useState(initial);
  // The wizard is keyed by pairing, so its transport and draft share one lifetime.
  const [session] = useState(() => createPhoneSession(pairing, render));
  const { connect, ...commands } = session;
  useEffect(() => connect(), [connect]);
  return { ...state, ...commands };
}
