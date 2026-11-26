import {
  isAnyOf,
  type TypedStartListening,
  type ThunkDispatch,
  type UnknownAction,
} from '@reduxjs/toolkit';
import {
  failureOf,
  reconnectDelayMs,
  streamStep,
  type ApplicationClient,
  type Failure,
  type Observation,
} from '@prysm/application-client';
import type { ApplicationActions, ApplicationState } from './slice';

export type ApplicationDispatch<C, O> = ThunkDispatch<
  ApplicationState<C, O>,
  ApplicationClient<C, O>,
  UnknownAction
>;

/**
 * Owns the state stream: connects on request, feeds every state to the slice
 * and, after a failure, does its {@link streamStep}: reconnects after
 * {@link reconnectDelayMs}, or stops. It also stops on release.
 * Closing the stream never commands lighting.
 */
export function registerObservation<C, O>(
  startListening: TypedStartListening<
    ApplicationState<C, O>,
    ApplicationDispatch<C, O>,
    ApplicationClient<C, O>
  >,
  actions: ApplicationActions<C, O>,
) {
  startListening({
    matcher: isAnyOf(actions.observationRequested, actions.observationReleased),
    effect: async (action, api) => {
      api.cancelActiveListeners();
      if (actions.observationReleased.match(action)) return;
      const generation = api.getState().connection.generation;
      let attempt = 0;
      while (!api.signal.aborted) {
        let observation: Observation<C, O> | undefined;
        const close = () => {
          const active = observation;
          observation = undefined;
          active?.close();
        };
        api.signal.addEventListener('abort', close, { once: true });
        // A stream that ends without a failure lost its connection.
        let failure: Failure = {
          kind: 'unavailable',
          message: 'Connection to Prysm closed.',
        };
        try {
          observation = await api.extra.observe(api.signal);
          if (api.signal.aborted) break;
          api.dispatch(
            actions.snapshotReceived({
              generation,
              snapshot: observation.initial,
            }),
          );
          attempt = 0;
          for await (const snapshot of observation.updates) {
            if (api.signal.aborted) break;
            api.dispatch(actions.snapshotReceived({ generation, snapshot }));
          }
        } catch (error) {
          failure = failureOf(error);
        } finally {
          api.signal.removeEventListener('abort', close);
          close();
        }
        if (api.signal.aborted) return;
        api.dispatch(actions.observationLost({ generation, failure }));
        // A kind newer than this page is not known to pass: it stops.
        if (streamStep[failure.kind] !== 'reconnect') return;
        await api.delay(reconnectDelayMs(attempt));
        attempt++;
        api.dispatch(actions.observationRetrying(generation));
      }
    },
  });
}
