import {
  isAnyOf,
  type TypedStartListening,
  type ThunkDispatch,
  type UnknownAction,
} from '@reduxjs/toolkit';
import {
  ApplicationError,
  type ApplicationClient,
  type Observation,
} from '@prysm/application-client';
import type { ApplicationActions, ApplicationState } from './slice';

export type ApplicationDispatch<C, O> = ThunkDispatch<
  ApplicationState<C, O>,
  ApplicationClient<C, O>,
  UnknownAction
>;

/**
 * Owns the state stream: connects on request, feeds every state to the slice,
 * reconnects with a bounded backoff and stops on release or when the portal
 * refuses this page (`forbidden`).
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
      let delay = 250;
      while (!api.signal.aborted) {
        let observation: Observation<C, O> | undefined;
        const close = () => {
          const active = observation;
          observation = undefined;
          active?.close();
        };
        api.signal.addEventListener('abort', close, { once: true });
        try {
          observation = await api.extra.observe(api.signal);
          if (api.signal.aborted) break;
          api.dispatch(
            actions.snapshotReceived({
              generation,
              snapshot: observation.initial,
            }),
          );
          delay = 250;
          for await (const snapshot of observation.updates) {
            if (api.signal.aborted) break;
            api.dispatch(actions.snapshotReceived({ generation, snapshot }));
          }
          if (!api.signal.aborted)
            api.dispatch(
              actions.observationLost({
                generation,
                message: 'Connection to Prysm closed.',
              }),
            );
        } catch (error) {
          if (!api.signal.aborted)
            api.dispatch(
              actions.observationLost({
                generation,
                message: error instanceof Error ? error.message : String(error),
              }),
            );
          if (
            error instanceof ApplicationError &&
            error.failure.kind === 'forbidden'
          )
            return;
        } finally {
          api.signal.removeEventListener('abort', close);
          close();
        }
        if (api.signal.aborted) return;
        await api.delay(delay);
        delay = Math.min(delay * 2, 5000);
        api.dispatch(actions.observationRetrying(generation));
      }
    },
  });
}
