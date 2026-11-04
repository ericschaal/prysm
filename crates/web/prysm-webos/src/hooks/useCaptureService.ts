import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import {
  createServiceClient,
  parseReply,
  parseSetupReply,
  type Config,
  type Status,
  type Reply,
  type ServiceBridge,
} from '../service';

import {
  embeddedBuild,
  sameOutput,
  supportsEmbedded,
} from '../processingLocation';

const defaults: Config = {
  instanceUrl: null,
  source: 'both',
  compression: 'lz4',
  autostart: false,
  metrics: false,
};
type Method = 'setConfig' | 'start' | 'stop' | 'cleanup';
interface State {
  online: boolean | null;
  status: Status | null;
  edits: Partial<Config>;
  changing: boolean;
  notice: string;
  error: boolean;
}
type Action =
  | {
      type: 'observe';
      status: Status;
      method?: Method;
    }
  | { type: 'offline'; text: string }
  | { type: 'edit'; patch: Partial<Config> }
  | { type: 'notice'; text: string; error: boolean }
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
    let equal = remaining[key] === saved[key];
    if (key === 'embeddedOutput')
      equal = sameOutput(remaining.embeddedOutput, saved.embeddedOutput);
    if (key === 'location')
      equal = (remaining.location ?? 'remote') === (saved.location ?? 'remote');
    if (equal) delete remaining[key];
  }
  return remaining;
}

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case 'observe': {
      const { status } = action;
      const edits = unsaved(state.edits, status.config);
      if (!supportsEmbedded(status)) {
        delete edits.location;
        delete edits.embeddedOutput;
      }
      if (action.method === 'cleanup') delete edits.autostart;
      return {
        ...state,
        online: true,
        status,
        edits: action.method === 'setConfig' ? {} : edits,
        notice:
          !action.method && state.online === false
            ? 'Service connected.'
            : state.notice,
        error: !action.method && state.online === false ? false : state.error,
      };
    }
    case 'offline':
      return { ...state, online: false, error: true, notice: action.text };
    case 'edit':
      return {
        ...state,
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
      };
    case 'changing':
      return { ...state, changing: action.value };
  }
}

type OpenSetup = (dialog: 'receiver' | 'wled') => void;

export function useCaptureService() {
  const [client] = useState(createServiceClient);
  const call = client.call;
  const [state, dispatch] = useReducer(reducer, initialState);
  const latest = useRef<Status | null>(null);
  const observe = useCallback((reply: Status, method?: Method) => {
    const current = latest.current;
    if (method && current && current.pid !== reply.pid) return false;
    const status =
      current?.pid === reply.pid && current.revision > reply.revision
        ? current
        : reply;
    if (
      method &&
      status !== reply &&
      (Object.keys(unsaved(reply.config, status.config)).length ||
        Object.keys(unsaved(status.config, reply.config)).length)
    ) {
      return false;
    }
    latest.current = status;
    dispatch({ type: 'observe', status, method });
    return true;
  }, []);
  const busy = useRef(false);
  const reconnect = useRef(() => {});
  const { online, status, edits } = state;
  const saved = status?.config ?? null;
  const draft = { ...(saved ?? defaults), ...edits };
  const embeddedAvailable = supportsEmbedded(status);
  // If a standard UI attaches to an embedded service, save remote mode before starting.
  if (!embeddedAvailable && draft.location === 'embedded')
    draft.location = 'remote';
  const dirty = !saved || Object.keys(unsaved(draft, saved)).length > 0;
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
        observe(value);
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
  }, [client, observe]);

  function request(method: Method, enableAfterSave = false) {
    if (busy.current) return;
    busy.current = true;
    dispatch({ type: 'changing', value: true });
    call(
      'luna://io.prysm.capture.service/' + method,
      method === 'setConfig' ? draft : {},
      parseReply,
      (value) => {
        busy.current = false;
        dispatch({ type: 'changing', value: false });
        if (!value.returnValue) {
          notice(
            value.errorText || 'The service could not complete this action.',
            true,
          );
          return;
        }
        if (!observe(value, method)) return;
        if (method === 'setConfig') {
          notice('Settings saved.');
          if (enableAfterSave) request('start');
        } else if (method === 'stop') {
          notice('');
        } else if (method === 'cleanup') {
          notice('Service setup removed. Receiver settings kept.');
        }
      },
    );
  }
  function missingSetup(): 'receiver' | 'wled' | null {
    if (embeddedBuild && embeddedAvailable && draft.location === 'embedded') {
      return draft.embeddedOutput ? null : 'wled';
    }
    return draft.instanceUrl ? null : 'receiver';
  }
  function saveSettings(openSetup: OpenSetup) {
    const missing = missingSetup();
    if (missing) {
      notice(
        missing === 'receiver'
          ? 'Add the Prysm instance URL first.'
          : 'Set the WLED address and LED counts first.',
        true,
      );
      openSetup(missing);
      return;
    }
    request('setConfig');
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
  function primary(openSetup: OpenSetup) {
    if (busy.current) return;
    if (!online) {
      setup();
      return;
    }
    if (enabled) {
      request('stop');
      return;
    }
    const missing = missingSetup();
    if (missing) {
      openSetup(missing);
      return;
    }
    if (dirty) request('setConfig', true);
    else request('start');
  }

  return {
    ...state,
    saved,
    embeddedAvailable,
    draft,
    dirty,
    edit,
    saveSettings,
    primary,
    setup,
    cleanup: () => request('cleanup'),
  };
}
