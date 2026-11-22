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
  style: { kind: 'natural' as const },
  sample_density: 1,
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
    version: '1.0.0',
    target: 'off',
    session: { state: 'stopped' },
    device: {},
    settings: {
      ...(initial.capture === undefined ? {} : { capture: initial.capture }),
      ...(initial.output === undefined ? {} : { output: initial.output }),
      processing: defaultProcessing,
    },
    revision: initial.revision ?? 0,
    surface_failures: [],
  };
  const subscribers = new Set<(value: ApplicationSnapshot<C, O>) => void>();
  /** Told every state, as a worker on the same engine is (`watch`). */
  const watchers = new Set<(value: ApplicationSnapshot<C, O>) => void>();
  const calls: string[] = [];
  /** Ends each open stream, as a lost connection does. */
  const streams = new Set<() => void>();
  /** The next command fails with this instead of running. */
  let nextFailure: Failure | null = null;
  /** Trials wait for this before answering; trials fail while `trialFailure`. */
  let trialGate: Promise<void> | null = null;
  let saveGate: Promise<void> | null = null;
  let patternGate: Promise<void> | null = null;
  let trialFailure: Failure | null = null;
  const trials = { inFlight: 0, mostInFlight: 0 };
  function publish(change: Partial<ApplicationSnapshot<C, O>>) {
    snapshot = { ...snapshot, ...change };
    for (const changed of subscribers) changed(snapshot);
    for (const watcher of watchers) watcher(snapshot);
  }
  /** As the engine: calibration ends when the target leaves `screen_sync`. */
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
        streams.delete(close);
        signal.removeEventListener('abort', close);
        wake?.();
      }
      subscribers.add(push);
      streams.add(close);
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
      // As the engine: saves are taken in order, each against the then
      // current revision.
      while (saveGate) await saveGate;
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
    // As the engine: lights with it, changes no state and saves nothing.
    tryProcessing: async (processing) => {
      calls.push(`trial:${processing.brightness_percent}`);
      trials.mostInFlight = Math.max(trials.mostInFlight, ++trials.inFlight);
      try {
        await trialGate;
        if (trialFailure) throw new ApplicationError(trialFailure);
      } finally {
        trials.inFlight--;
      }
    },
    setTarget: async (target) => {
      calls.push(`target:${target}`);
      failIfScripted();
      const { capture, output } = snapshot.settings;
      let missing: 'output' | 'capture' | null = null;
      if (target !== 'off' && !output) missing = 'output';
      else if (target === 'screen_sync' && !capture) missing = 'capture';
      if (missing) {
        // As the engine: the reply, and the session's failure.
        const failure = {
          kind: 'invalid',
          field: missing,
          message: 'not configured',
        } as const;
        // A start of screen sync from the effects leaves them on.
        const fromEffects =
          target === 'screen_sync' && snapshot.session.state === 'effects';
        publish({
          target,
          session: fromEffects
            ? { state: 'effects', failure }
            : { state: 'stopped', failure },
        });
        throw new ApplicationError(failure);
      }
      if (target !== 'screen_sync') clearCalibration();
      publish({
        target,
        session: { state: target === 'off' ? 'stopped' : target },
      });
    },
    setCalibration: async (calibration) => {
      calls.push(
        calibration
          ? `calibration:${calibration.pattern}${calibration.uncorrected ? ':uncorrected' : ''}`
          : 'calibration:end',
      );
      // As a TV on Wi-Fi: the answer takes a while.
      while (patternGate) await patternGate;
      failIfScripted();
      if (!calibration) {
        clearCalibration();
        publish({});
      } else if (snapshot.target !== 'screen_sync') {
        throw new ApplicationError({
          kind: 'needs_screen_sync',
          message: 'calibration patterns need screen sync: turn it on first',
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
    /**
     * Call `listener` with the current state and every later one, as the
     * engine's own snapshot watch (the TV worker's); returns the unwatch.
     */
    watch: (listener: (value: ApplicationSnapshot<C, O>) => void) => {
      watchers.add(listener);
      listener(snapshot);
      return () => {
        watchers.delete(listener);
      };
    },
    /** Lose the connection: every open stream ends; the client reconnects. */
    drop: () => {
      for (const close of [...streams]) close();
    },
    failNext: (failure: Failure) => {
      nextFailure = failure;
    },
    trials,
    /** Trials fail with `failure` until called with `null`. */
    failTrials: (failure: Failure | null) => {
      trialFailure = failure;
    },
    /** Saves wait, in order, until the returned function is called. */
    holdSaves: () => {
      let release = () => {};
      saveGate = new Promise<void>((resolve) => {
        release = () => {
          saveGate = null;
          resolve();
        };
      });
      return release;
    },
    /** Calibration patterns wait, in order, until the returned function is called. */
    holdPatterns: () => {
      let release = () => {};
      patternGate = new Promise<void>((resolve) => {
        release = () => {
          patternGate = null;
          resolve();
        };
      });
      return release;
    },
    /** Trials stay unanswered until the returned function is called. */
    holdTrials: () => {
      let release = () => {};
      trialGate = new Promise<void>((resolve) => {
        release = () => {
          trialGate = null;
          resolve();
        };
      });
      return release;
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
