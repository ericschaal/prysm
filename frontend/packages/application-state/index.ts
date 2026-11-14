import { configureStore, createListenerMiddleware } from '@reduxjs/toolkit';
import type { ApplicationClient } from '@prysm/application-client';
import { createApplicationSlice, type ApplicationState } from './slice';
import { createCommands } from './commands';
import { registerObservation, type ApplicationDispatch } from './observation';

export type {
  ApplicationState,
  CommandKind,
  CommandState,
  Connection,
} from './slice';

/** Host-owned frontend assembly: one Redux store and one injected transport client. */
export function createApplicationRuntime<C, O>(
  client: ApplicationClient<C, O>,
) {
  const slice = createApplicationSlice<C, O>();
  const listener = createListenerMiddleware<
    ApplicationState<C, O>,
    ApplicationDispatch<C, O>,
    ApplicationClient<C, O>
  >({ extra: client });
  registerObservation(listener.startListening, slice.actions);
  const store = configureStore({
    reducer: slice.reducer,
    middleware: (defaults) =>
      defaults({ thunk: { extraArgument: client } }).prepend(
        listener.middleware,
      ),
  });
  // Subscription leases are resources, not application data; the last release ends observation only.
  let consumers = 0;
  function connect() {
    if (consumers++ === 0) store.dispatch(slice.actions.observationRequested());
    let released = false;
    return () => {
      if (released) return;
      released = true;
      if (--consumers === 0)
        store.dispatch(slice.actions.observationReleased());
    };
  }
  return {
    store,
    client,
    actions: slice.actions,
    commands: createCommands(slice.actions),
    selectors: slice.getSelectors(),
    connect,
  };
}

export type ApplicationRuntime<C, O> = ReturnType<
  typeof createApplicationRuntime<C, O>
>;
