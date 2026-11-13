/* eslint-disable @typescript-eslint/require-await -- Fixtures implement async ports without I/O; thrown errors must reject promises. */
import { createApplicationRuntime } from '@prysm/application-state';
import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type Failure,
} from '@prysm/application-client';

export const defaultProcessing = {
  brightness_percent: 100,
  shadow_lift_percent: 0,
  smoothing_seconds: 0,
  sample_density: 1,
  spatial_reach_percent: 10,
  remove_black_bars: false,
};

/**
 * In-memory application: revision checks, immediate state updates and
 * scriptable failures. No hardware, persistence, network or engine timing.
 */
export function fakeApplicationPort<C, O>(initial: {
  capture?: C;
  output?: O;
  revision?: number;
}) {
  let snapshot: ApplicationSnapshot<C, O> = {
    target: 'off',
    session: { state: 'stopped' },
    settings: {
      ...(initial.capture === undefined ? {} : { capture: initial.capture }),
      ...(initial.output === undefined ? {} : { output: initial.output }),
      processing: defaultProcessing,
    },
    revision: initial.revision ?? 0,
    failure: null,
    surface_failures: [],
  };
  const subscribers = new Set<(value: ApplicationSnapshot<C, O>) => void>();
  const calls: string[] = [];
  /** The next command fails with this instead of running. */
  let nextFailure: Failure | null = null;
  function publish(change: Partial<ApplicationSnapshot<C, O>>) {
    snapshot = { ...snapshot, ...change };
    for (const changed of subscribers) changed(snapshot);
  }
  /** As the engine: calibration ends when the target leaves `processed`. */
  function clearCalibration() {
    snapshot = { ...snapshot };
    delete snapshot.calibration;
  }
  function failIfScripted() {
    const failure = nextFailure;
    nextFailure = null;
    if (failure) throw new ApplicationError(failure);
  }
  const client: ApplicationClient<C, O> = {
    observe: async (signal) => {
      calls.push('observe');
      const queue: ApplicationSnapshot<C, O>[] = [];
      let wake: (() => void) | undefined;
      let closed = false;
      const push = (value: ApplicationSnapshot<C, O>) => {
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
    replaceSettings: async (expected, settings) => {
      calls.push('replaceSettings');
      failIfScripted();
      if (expected !== snapshot.revision)
        throw new ApplicationError({
          kind: 'stale_revision',
          message: `settings revision ${expected} is stale; current is ${snapshot.revision}`,
          current: snapshot.revision,
        });
      publish({ settings, revision: snapshot.revision + 1 });
      return snapshot.revision;
    },
    setTarget: async (target) => {
      calls.push(`target:${target}`);
      failIfScripted();
      const { capture, output } = snapshot.settings;
      const missing =
        target === 'off' ? null : !output ? 'output' : target === 'processed' && !capture ? 'capture' : null;
      if (missing) {
        const failure: Failure = { kind: 'invalid', field: missing, message: 'not configured' };
        publish({ target, session: { state: 'stopped' }, failure });
        throw new ApplicationError(failure);
      }
      if (target !== 'processed') clearCalibration();
      publish({
        target,
        failure: null,
        session: {
          state: { off: 'stopped', processed: 'running', device_native: 'native' }[target] as
            'stopped' | 'running' | 'native',
        },
      });
    },
    setCalibration: async (calibration) => {
      calls.push(
        calibration
          ? `calibration:${calibration.pattern}${calibration.uncorrected ? ':uncorrected' : ''}`
          : 'calibration:end',
      );
      failIfScripted();
      if (!calibration) {
        clearCalibration();
        publish({});
      } else if (snapshot.target !== 'processed') {
        throw new ApplicationError({
          kind: 'not_processed',
          message: 'calibration patterns need lighting on: set the target to processed first',
        });
      } else publish({ calibration });
    },
  };
  return {
    client,
    calls,
    snapshot: () => snapshot,
    /** Simulate another client or the engine changing state. */
    publish,
    failNext: (failure: Failure) => {
      nextFailure = failure;
    },
  };
}

export function fakeApplication<C, O>(initial: {
  capture?: C;
  output?: O;
  revision?: number;
}) {
  return createApplicationRuntime(fakeApplicationPort(initial).client);
}
