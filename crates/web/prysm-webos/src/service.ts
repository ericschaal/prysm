import {
  type components,
  isErrorReply,
  isExecReply,
  isStatus,
} from './service.generated.js';

export type Config = components['schemas']['Config'];
export type Status = components['schemas']['StatusReply'];
type Failure = components['schemas']['FailureReply'];
export type Reply = Status | Failure;

export interface ServiceBridge {
  onservicecallback: (text: string) => void;
  call(uri: string, payload: string): void;
  cancel(): void;
}

declare global {
  var PalmServiceBridge: { new (): ServiceBridge };
}

const unavailable =
  'Open Prysm Capture on your LG TV to connect to the service.';
const timedOut =
  'The service did not respond. Open Service options to repair setup.';

export function parseReply(text: string): Reply {
  const value: unknown = JSON.parse(text);
  if (isStatus(value) || isErrorReply(value)) return value;
  throw new Error('The service returned an invalid reply.');
}

export function parseSetupReply(text: string): { returnValue: true } | Failure {
  const value: unknown = JSON.parse(text);
  if (!isExecReply(value))
    throw new Error('Homebrew Channel returned an invalid reply.');
  if (!value.returnValue) {
    return {
      returnValue: false,
      errorText:
        value.errorText || value.stderrString || 'Service setup failed.',
    };
  }
  return { returnValue: true };
}

function errorReply(error: unknown): Failure {
  return {
    returnValue: false,
    errorText: error instanceof Error ? error.message : String(error),
  };
}

// Keep native bridges alive until completion, scoped to the mounted consumer.
export function createServiceClient() {
  const pending = new Set<() => void>();
  function call<T>(
    uri: string,
    payload: object,
    parse: (text: string) => T,
    callback: (value: T | Failure) => void,
  ) {
    if (typeof PalmServiceBridge === 'undefined') {
      callback({ returnValue: false, errorText: unavailable });
      return;
    }
    let bridge: ServiceBridge;
    try {
      bridge = new PalmServiceBridge();
    } catch (error) {
      callback(errorReply(error));
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      cancel();
      callback({ returnValue: false, errorText: timedOut });
    }, 30000);
    function release() {
      active = false;
      clearTimeout(timer);
      pending.delete(cancel);
    }
    function cancel() {
      if (!active) return;
      release();
      bridge.cancel();
    }
    function done(value: T | Failure) {
      if (!active) return;
      release();
      callback(value);
    }
    pending.add(cancel);
    bridge.onservicecallback = (text) => {
      let value: T | Failure;
      try {
        value = parse(text);
      } catch (error) {
        value = errorReply(error);
      }
      done(value);
    };
    try {
      bridge.call(uri, JSON.stringify(payload));
    } catch (error) {
      done(errorReply(error));
    }
  }
  return {
    call,
    cancelAll: () => {
      for (const cancel of pending) cancel();
    },
  };
}
