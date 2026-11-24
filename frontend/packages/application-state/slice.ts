import { createSlice, type Draft, type PayloadAction } from '@reduxjs/toolkit';
import type { ApplicationSnapshot, Failure } from '@prysm/application-client';

export interface Connection<C, O> {
  generation: number;
  status: 'connecting' | 'connected' | 'disconnected';
  /**
   * The newest state of this stream (`generation`), or the state the page
   * came with until the stream sends one; kept while it reconnects, current
   * only while `status` is `connected`.
   */
  snapshot: ApplicationSnapshot<C, O> | null;
  /** Why the stream was lost; kept while it reconnects. */
  error: Failure | null;
}

export interface ApplicationState<C, O> {
  connection: Connection<C, O>;
}

export function createApplicationSlice<C, O>(
  initial: ApplicationSnapshot<C, O> | null,
) {
  const initialState: ApplicationState<C, O> = {
    connection: {
      generation: 0,
      // A state the page came with is current, as a streamed one is.
      status: initial ? 'connected' : 'disconnected',
      snapshot: initial,
      error: null,
    },
  };
  const current = (
    state: { connection: { generation: number } },
    generation: number,
  ) => state.connection.generation === generation;
  return createSlice({
    name: 'application',
    initialState,
    reducers: {
      /**
       * The first request after the start or a release; only a state the
       * page came with is there, and it stays current until the stream's
       * first replaces it.
       */
      observationRequested(state) {
        state.connection.generation++;
        if (!state.connection.snapshot) state.connection.status = 'connecting';
        state.connection.error = null;
      },
      observationReleased(state) {
        state.connection.generation++;
        state.connection.status = 'disconnected';
        state.connection.snapshot = null;
        state.connection.error = null;
      },
      observationRetrying(state, action: PayloadAction<number>) {
        if (current(state, action.payload))
          state.connection.status = 'connecting';
      },
      /** Every state is complete; the newest one simply replaces the last. */
      snapshotReceived(
        state,
        action: PayloadAction<{
          generation: number;
          snapshot: ApplicationSnapshot<C, O>;
        }>,
      ) {
        if (
          !current(state, action.payload.generation) ||
          state.connection.status === 'disconnected'
        )
          return;
        state.connection.status = 'connected';
        state.connection.error = null;
        state.connection.snapshot = action.payload.snapshot as Draft<
          ApplicationSnapshot<C, O>
        >;
      },
      observationLost(
        state,
        action: PayloadAction<{ generation: number; failure: Failure }>,
      ) {
        if (!current(state, action.payload.generation)) return;
        state.connection.status = 'disconnected';
        state.connection.error = action.payload.failure;
      },
    },
    selectors: {
      selectConnection: (state) => state.connection,
    },
  });
}

export type ApplicationActions<C, O> = ReturnType<
  typeof createApplicationSlice<C, O>
>['actions'];
