import {
  isAnyOf,
  type TypedStartListening,
  type ThunkDispatch,
  type UnknownAction,
} from '@reduxjs/toolkit';
import type { ApplicationClient, Observation } from '@prysm/application-client';
import type { ApplicationActions, ApplicationState } from './slice';

export type ApplicationDispatch<C, O> = ThunkDispatch<
  ApplicationState<C, O>,
  ApplicationClient<C, O>,
  UnknownAction
>;

/** Single registration point for observation effects; connections never enter Redux state. */
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
      let observation: Observation<C, O> | undefined;
      const close = () => {
        const current = observation;
        observation = undefined;
        current?.close();
      };
      api.signal.addEventListener('abort', close, { once: true });
      try {
        const active = await api.extra.observe(api.signal);
        observation = active;
        if (api.signal.aborted) return;
        api.dispatch(
          actions.observationConnected({
            generation,
            snapshot: active.initial,
          }),
        );
        for await (const update of active.updates) {
          if (api.signal.aborted) return;
          if (update.kind === 'resync_required') {
            api.dispatch(actions.resyncRequired(generation));
            return;
          }
          api.dispatch(
            actions.snapshotReceived({ generation, snapshot: update.snapshot }),
          );
          if (api.getState().connection.status !== 'connected') return;
        }
        if (!api.signal.aborted)
          api.dispatch(
            actions.observationLost({
              generation,
              message: 'Application connection closed.',
            }),
          );
      } catch (error) {
        if (!api.signal.aborted)
          api.dispatch(
            actions.observationLost({ generation, message: String(error) }),
          );
      } finally {
        api.signal.removeEventListener('abort', close);
        close();
      }
    },
  });
}
