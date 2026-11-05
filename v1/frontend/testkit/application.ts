import { createApplicationRuntime } from '@prysm/application-state';
/* eslint-disable @typescript-eslint/require-await -- Fixtures implement async ports without I/O; thrown errors must reject promises. */
import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type ApplicationUpdate,
  type OperationId,
  type OperationStatus,
} from '@prysm/application-client';

export function fakeApplicationPort<C, O>(
  initial: Pick<ApplicationSnapshot<C, O>, 'capture' | 'output' | 'revision'>,
  instance = '1',
) {
  let snapshot: ApplicationSnapshot<C, O> = {
    ...initial,
    cursor: { instance, sequence: '0' },
    target: 'off',
  };
  const operations = new Map<string, OperationStatus>();
  const subscribers = new Set<(value: ApplicationUpdate<C, O>) => void>();
  const calls: string[] = [];
  const key = (id: OperationId) => `${id.instance}/${id.request}`;
  function accept(
    id: OperationId,
    change: Partial<ApplicationSnapshot<C, O>>,
    expected?: string,
  ) {
    if (id.instance === instance && operations.has(key(id)))
      return { operation: id };
    if (
      id.instance !== instance ||
      (expected !== undefined && expected !== snapshot.revision)
    ) {
      throw new ApplicationError({
        kind: 'rejected',
        message: 'Settings revision or application instance conflict.',
      });
    }
    snapshot = {
      ...snapshot,
      ...change,
      cursor: {
        instance,
        sequence: (BigInt(snapshot.cursor.sequence) + 1n).toString(),
      },
    };
    operations.set(key(id), { kind: 'succeeded' });
    for (const changed of subscribers) changed({ kind: 'snapshot', snapshot });
    return { operation: id };
  }
  const client: ApplicationClient<C, O> = {
    observe: async (signal) => {
      calls.push('observe');
      const queue: ApplicationUpdate<C, O>[] = [];
      let wake: (() => void) | undefined;
      let closed = false;
      const push = (value: ApplicationUpdate<C, O>) => {
        queue.push(value);
        wake?.();
      };
      function close() {
        if (closed) return;
        closed = true;
        calls.push('unsubscribe');
        subscribers.delete(push);
        signal.removeEventListener('abort', close);
        wake?.();
      }
      subscribers.add(push);
      signal.addEventListener('abort', close, { once: true });
      if (signal.aborted) close();
      return {
        initial: snapshot,
        close,
        updates: (async function* () {
          while (!closed) {
            if (!queue.length)
              await new Promise<void>((resolve) => {
                wake = resolve;
              });
            if (closed) return;
            while (queue.length) yield queue.shift()!;
          }
        })(),
      };
    },
    configureCapture: async ({ id, expected, settings }) => {
      calls.push('configureCapture');
      return accept(
        id,
        { capture: settings, revision: (BigInt(expected) + 1n).toString() },
        expected,
      );
    },
    configureOutput: async ({ id, expected, settings }) => {
      calls.push('configureOutput');
      return accept(
        id,
        { output: settings, revision: (BigInt(expected) + 1n).toString() },
        expected,
      );
    },
    setTarget: async ({ id, target }) => {
      calls.push(`target:${target}`);
      return accept(id, { target });
    },
    operation: async (id) => {
      const result = operations.get(key(id));
      if (!result) throw new ApplicationError({ kind: 'unknown_operation' });
      return result;
    },
    calibration: {
      open: async () => {
        calls.push('calibration:open');
        return {
          session: 'fixture-private-session',
          pairing: 'fixture-private-pairing',
          pattern: { id: 'fixture-gray' },
        };
      },
      feedback: async ({ playback }) => {
        calls.push(`calibration:${playback.kind}`);
      },
      close: async () => {
        calls.push('calibration:close');
      },
    },
  };
  return {
    client,
    calls,
    snapshot: () => snapshot,
    resync: () => {
      for (const changed of subscribers) changed({ kind: 'resync_required' });
    },
  };
}

/** In-memory fixture; no hardware validation, persistence, network or coordinator scheduler. */
export function fakeApplication<C, O>(
  initial: Pick<ApplicationSnapshot<C, O>, 'capture' | 'output' | 'revision'>,
) {
  return createApplicationRuntime(fakeApplicationPort(initial).client);
}
