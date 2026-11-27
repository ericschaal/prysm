import {
  ApplicationError,
  consumeStream,
  failureOf,
  reconnectDelayMs,
  terminalStreamFailure,
  type ApplicationClient,
  type ApplicationSnapshot,
  type CalibrationPattern,
  type LightingTarget,
  type Failure,
  type Settings,
} from '@prysm/application-client';

export interface Connection<C, O> {
  status: 'connecting' | 'connected' | 'disconnected';
  snapshot: ApplicationSnapshot<C, O> | null;
  error: Failure | null;
}

export function createApplicationRuntime<C, O>(
  client: ApplicationClient<C, O>,
  initial: ApplicationSnapshot<C, O> | null = null,
) {
  let current: Connection<C, O> = {
    status: initial ? 'connected' : 'disconnected',
    snapshot: initial,
    error: null,
  };
  const listeners = new Set<() => void>();
  let consumers = 0;
  let owner: AbortController | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;

  function publish(connection: Connection<C, O>) {
    current = connection;
    for (const notify of listeners) notify();
  }

  async function observe(signal: AbortSignal, attempt: number) {
    let failure: Failure = {
      kind: 'unavailable',
      message: 'Connection to Prysm closed.',
    };
    try {
      await consumeStream(client.observe(signal), signal, (snapshot) => {
        publish({ status: 'connected', snapshot, error: null });
        attempt = 0;
      });
    } catch (error) {
      failure = failureOf(error);
    }
    if (signal.aborted) return;
    publish({ ...current, status: 'disconnected', error: failure });
    if (!signal.aborted && !terminalStreamFailure(failure)) {
      retry = setTimeout(() => {
        publish({ ...current, status: 'connecting' });
        if (!signal.aborted) void observe(signal, attempt + 1);
      }, reconnectDelayMs(attempt));
    }
  }

  function connect() {
    if (++consumers === 1) {
      const controller = new AbortController();
      owner = controller;
      publish({
        ...current,
        status: current.snapshot ? 'connected' : 'connecting',
        error: null,
      });
      void observe(controller.signal, 0);
    }
    let released = false;
    return () => {
      if (released) return;
      released = true;
      if (--consumers === 0) {
        owner?.abort();
        owner = undefined;
        clearTimeout(retry);
        publish({ status: 'disconnected', snapshot: null, error: null });
      }
    };
  }

  async function replaceSections(
    expected: number,
    sections: Partial<Settings<C, O>>,
  ) {
    const { snapshot, status } = current;
    if (status !== 'connected' || !snapshot || snapshot.revision !== expected)
      throw new ApplicationError({
        kind: 'stale_revision',
        message: 'Settings changed; review them before saving again.',
        current: snapshot?.revision ?? expected,
      });
    return client.replaceSettings(expected, {
      ...snapshot.settings,
      ...sections,
    });
  }

  return {
    getConnection: () => current,
    subscribe: (changed: () => void) => {
      listeners.add(changed);
      return () => {
        listeners.delete(changed);
      };
    },
    connect,
    commands: {
      setTarget: (target: LightingTarget) => client.setTarget(target),
      setCalibration: (pattern: CalibrationPattern | null) =>
        client.setCalibration(pattern),
      tryProcessing: (processing: Settings<C, O>['processing']) =>
        client.tryProcessing(processing),
      configureCapture: (expected: number, capture: C) =>
        replaceSections(expected, { capture }),
      configureOutput: (expected: number, output: O) =>
        replaceSections(expected, { output }),
      configureSetup: (expected: number, capture: C, output: O) =>
        replaceSections(expected, { capture, output }),
      configureProcessing: (
        expected: number,
        processing: Settings<C, O>['processing'],
      ) => replaceSections(expected, { processing }),
    },
  };
}

export type ApplicationRuntime<C, O> = ReturnType<
  typeof createApplicationRuntime<C, O>
>;
