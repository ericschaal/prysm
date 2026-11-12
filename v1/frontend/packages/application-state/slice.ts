import { createSlice, type Draft, type PayloadAction } from '@reduxjs/toolkit';
import type { ApplicationSnapshot, Failure } from '@prysm/application-client';

export interface Connection<C, O> {
  generation: number;
  status: 'connecting' | 'connected' | 'disconnected';
  snapshot: ApplicationSnapshot<C, O> | null;
  error: string | null;
}

export type CommandKind =
  | 'configureCapture'
  | 'configureOutput'
  | 'configureProcessing'
  | 'setTarget';
export interface CommandState {
  /** Local sequence number; only the latest command is shown. */
  id: number;
  kind: CommandKind;
  phase: 'submitting' | 'succeeded' | 'failed';
  failure: Failure | null;
}
export interface ApplicationState<C, O> {
  connection: Connection<C, O>;
  lastCommand: CommandState | null;
}

export function createApplicationSlice<C, O>() {
  const initialState: ApplicationState<C, O> = {
    connection: {
      generation: 0,
      status: 'disconnected',
      snapshot: null,
      error: null,
    },
    lastCommand: null,
  };
  const current = (
    state: { connection: { generation: number } },
    generation: number,
  ) => state.connection.generation === generation;
  return createSlice({
    name: 'application',
    initialState,
    reducers: {
      observationRequested(state) {
        state.connection.generation++;
        state.connection.status = 'connecting';
        state.connection.snapshot = null;
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
        action: PayloadAction<{ generation: number; message: string }>,
      ) {
        if (!current(state, action.payload.generation)) return;
        state.connection.status = 'disconnected';
        state.connection.snapshot = null;
        state.connection.error = action.payload.message;
      },
      commandSubmitted(
        state,
        action: PayloadAction<{ id: number; kind: CommandKind }>,
      ) {
        state.lastCommand = {
          ...action.payload,
          phase: 'submitting',
          failure: null,
        };
      },
      commandSucceeded(state, action: PayloadAction<number>) {
        if (state.lastCommand?.id === action.payload)
          state.lastCommand.phase = 'succeeded';
      },
      commandFailed(
        state,
        action: PayloadAction<{ id: number; failure: Failure }>,
      ) {
        if (state.lastCommand?.id !== action.payload.id) return;
        state.lastCommand.phase = 'failed';
        state.lastCommand.failure = action.payload.failure;
      },
    },
    selectors: {
      selectConnection: (state) => state.connection,
      selectSnapshot: (state) => state.connection.snapshot,
      selectLastCommand: (state) => state.lastCommand,
      selectCanCommand: (state) =>
        state.connection.status === 'connected' &&
        state.connection.snapshot !== null,
    },
  });
}

export type ApplicationActions<C, O> = ReturnType<
  typeof createApplicationSlice<C, O>
>['actions'];
