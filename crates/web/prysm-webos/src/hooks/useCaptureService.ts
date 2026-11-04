import { useEffect, useReducer, useRef, useState } from 'react';
import {
  createServiceClient,
  parseReply,
  parseSetupReply,
  type Config,
  type Status,
  type Reply,
  type ServiceBridge,
} from '../service';

const defaults: Config = {
  instanceUrl: '',
  source: 'both',
  compression: 'lz4',
  autostart: false,
  metrics: false,
};
interface SaveRequest {
  config: Config;
  enableAfterSave: boolean;
}
interface State {
  online: boolean | null;
  status: Status | null;
  edits: Partial<Config>;
  changing: boolean;
  notice: string;
  retry?: SaveRequest;
  error: boolean;
}
type Action =
  | {
      type: 'observe';
      status: Status;
      subscribed?: boolean;
      method?: 'setConfig' | 'start' | 'stop' | 'cleanup';
      requestPid?: number;
    }
  | { type: 'offline'; text: string }
  | { type: 'edit'; patch: Partial<Config> }
  | { type: 'notice'; text: string; error: boolean; retry?: SaveRequest }
  | { type: 'changing'; value: boolean };

const initialState: State = {
  online: null,
  status: null,
  edits: {},
  changing: false,
  notice: '',
  error: false,
};

function unsaved(edits: Partial<Config>, saved: Config) {
  const remaining = { ...edits };
  for (const key of Object.keys(remaining) as (keyof Config)[]) {
    if (remaining[key] === saved[key]) delete remaining[key];
  }
  return remaining;
}

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case 'observe': {
      const current = state.status;
      if (
        current &&
        action.requestPid === action.status.pid &&
        current.pid !== action.status.pid
      )
        return state;
      const status =
        current?.pid === action.status.pid &&
        current.revision > action.status.revision
          ? current
          : action.status;
      const edits = unsaved(state.edits, status.config);
      if (action.method === 'cleanup') delete edits.autostart;
      return {
        ...state,
        online: true,
        status,
        edits: action.method === 'setConfig' ? {} : edits,
        notice:
          action.subscribed && state.online === false
            ? 'Service connected.'
            : state.notice,
        error:
          action.subscribed && state.online === false ? false : state.error,
      };
    }
    case 'offline':
      return { ...state, online: false, error: true, notice: action.text };
    case 'edit':
      return {
        ...state,
        retry: undefined,
        edits: unsaved(
          { ...state.edits, ...action.patch },
          state.status?.config ?? defaults,
        ),
      };
    case 'notice':
      return {
        ...state,
        notice: action.text,
        error: action.error,
        retry: action.retry,
      };
    case 'changing':
      return { ...state, changing: action.value };
  }
}

export function useCaptureService(openReceiver: () => void) {
  const [client] = useState(createServiceClient);
  const call = client.call;
  const [state, dispatch] = useReducer(reducer, initialState);
  const busy = useRef(false);
  const reconnect = useRef(() => {});
  const { online, status, edits } = state;
  const saved = status?.config ?? null;
  const draft = { ...(saved ?? defaults), ...edits };
  const dirty = !saved || Object.keys(edits).length > 0;
  const enabled = status?.enabled;
  const notice = (text: string, error = false) =>
    dispatch({ type: 'notice', text, error });
  const edit = <K extends keyof Config>(key: K, value: Config[K]) =>
    dispatch({ type: 'edit', patch: { [key]: value } });

  useEffect(() => {
    let subscription: ServiceBridge | null = null;
    let retry: number | undefined;
    let timer: number | undefined;
    function stop() {
      const bridge = subscription;
      subscription = null;
      clearTimeout(retry);
      clearTimeout(timer);
      bridge?.cancel();
    }
    function failed(text?: string) {
      stop();
      dispatch({
        type: 'offline',
        text:
          text ||
          'The service is unavailable. Open Service options to repair setup.',
      });
      retry = window.setTimeout(watch, 3000);
    }
    function watch() {
      stop();
      if (typeof PalmServiceBridge === 'undefined') {
        failed('Open Prysm Capture on your LG TV to connect to the service.');
        return;
      }
      let bridge: ServiceBridge;
      try {
        bridge = subscription = new PalmServiceBridge();
      } catch (error) {
        failed(error instanceof Error ? error.message : String(error));
        return;
      }
      timer = window.setTimeout(
        () =>
          failed(
            'The service did not respond. Open Service options to repair setup.',
          ),
        30000,
      );
      bridge.onservicecallback = (text) => {
        if (subscription !== bridge) return;
        let value: Reply;
        try {
          value = parseReply(text);
        } catch (error) {
          failed(error instanceof Error ? error.message : String(error));
          return;
        }
        if (!value.returnValue) {
          failed(value.errorText);
          return;
        }
        if (!value.subscribed) {
          failed();
          return;
        }
        clearTimeout(timer);
        dispatch({
          type: 'observe',
          status: value,
          subscribed: true,
        });
      };
      try {
        bridge.call(
          'luna://io.prysm.capture.service/status',
          JSON.stringify({ subscribe: true }),
        );
      } catch (error) {
        failed(error instanceof Error ? error.message : String(error));
      }
    }
    const visibilityChanged = () => {
      if (document.hidden) stop();
      else watch();
    };
    reconnect.current = watch;
    watch();
    window.addEventListener('pagehide', stop);
    window.addEventListener('pageshow', watch);
    document.addEventListener('visibilitychange', visibilityChanged);
    return () => {
      stop();
      window.removeEventListener('pagehide', stop);
      window.removeEventListener('pageshow', watch);
      document.removeEventListener('visibilitychange', visibilityChanged);
      reconnect.current = () => {};
      client.cancelAll();
      busy.current = false;
    };
  }, [client]);

  function request(change: SaveRequest | 'start' | 'stop' | 'cleanup') {
    if (busy.current) return;
    busy.current = true;
    dispatch({ type: 'changing', value: true });
    const saving = typeof change === 'object';
    const method = saving ? 'setConfig' : change;
    const payload = saving ? change.config : {};
    call(
      'luna://io.prysm.capture.service/' + method,
      payload,
      parseReply,
      (value) => {
        busy.current = false;
        dispatch({ type: 'changing', value: false });
        if (!value.returnValue) {
          // An explicit :80 is also a port choice, even though URL normalizes it away.
          const host =
            saving &&
            value.retryDefaultPort &&
            /^http:\/\/(\[[^\]]+\]|[^/:?#@]+)\/?$/i.exec(
              change.config.instanceUrl.trim(),
            );
          if (host) {
            dispatch({
              type: 'notice',
              text: value.errorText || 'Could not reach Prysm.',
              error: true,
              retry: {
                ...change,
                config: {
                  ...change.config,
                  instanceUrl: `http://${host[1]}:8787`,
                },
              },
            });
            return;
          }
          notice(
            value.errorText || 'The service could not complete this action.',
            true,
          );
          return;
        }
        dispatch({
          type: 'observe',
          status: value,
          method,
          requestPid: status?.pid,
        });
        if (saving) {
          notice('Settings saved.');
          if (change.enableAfterSave) request('start');
        } else if (change === 'stop') {
          notice('');
        } else if (change === 'cleanup') {
          notice('Service setup removed. Receiver settings kept.');
        }
      },
    );
  }
  function saveSettings() {
    if (state.retry) {
      edit('instanceUrl', state.retry.config.instanceUrl);
      request(state.retry);
      return;
    }
    if (!draft.instanceUrl) {
      notice('Add the Prysm instance URL first.', true);
      openReceiver();
      return;
    }
    request({ config: draft, enableAfterSave: false });
  }
  function setup() {
    if (busy.current) return;
    busy.current = true;
    dispatch({ type: 'changing', value: true });
    notice('Setting up the service. This may take a moment.');
    call(
      'luna://org.webosbrew.hbchannel.service/exec',
      {
        command:
          '/media/developer/apps/usr/palm/services/io.prysm.capture.service/setup.sh',
      },
      parseSetupReply,
      (value) => {
        busy.current = false;
        dispatch({ type: 'changing', value: false });
        if (!value.returnValue) {
          notice(value.errorText, true);
        } else {
          notice('Service setup complete.');
          reconnect.current();
        }
      },
    );
  }
  function primary() {
    if (busy.current) return;
    if (!online) {
      setup();
      return;
    }
    if (enabled) {
      request('stop');
      return;
    }
    if (!draft.instanceUrl) {
      openReceiver();
      return;
    }
    if (dirty) request({ config: draft, enableAfterSave: true });
    else request('start');
  }

  return {
    ...state,
    saved,
    draft,
    dirty,
    edit,
    saveSettings,
    primary,
    setup,
    cleanup: () => request('cleanup'),
  };
}
