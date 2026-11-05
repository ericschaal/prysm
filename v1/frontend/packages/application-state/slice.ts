import { createSlice, type Draft, type PayloadAction } from '@reduxjs/toolkit';
import type {
  ApplicationSnapshot,
  ClientFailure,
  OperationId,
  OperationStatus,
} from '@prysm/application-client';

export interface Connection<C, O> {
  generation: number;
  status: 'connecting' | 'connected' | 'disconnected' | 'resync_required';
  snapshot: ApplicationSnapshot<C, O> | null;
  error: string | null;
}

export type CommandKind = 'configureCapture' | 'configureOutput' | 'setTarget';
export interface CommandState {
  id: OperationId;
  kind: CommandKind;
  phase: 'submitting' | 'accepted' | 'failed' | 'outcome_unknown';
  result: OperationStatus | null;
  failure: ClientFailure | null;
}
export interface ApplicationState<C, O> {
  connection: Connection<C, O>;
  // Latest command for UI feedback; retained operation history belongs to the coordinator.
  lastCommand: CommandState | null;
}

export function sameOperation(a: OperationId, b: OperationId) {
  return a.instance === b.instance && a.request === b.request;
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
      observationConnected(
        state,
        action: PayloadAction<{
          generation: number;
          snapshot: ApplicationSnapshot<C, O>;
        }>,
      ) {
        if (
          state.connection.generation !== action.payload.generation ||
          state.connection.status !== 'connecting'
        )
          return;
        state.connection.status = 'connected';
        state.connection.snapshot = action.payload.snapshot as Draft<
          ApplicationSnapshot<C, O>
        >;
        state.connection.error = null;
        if (
          state.lastCommand?.id.instance !==
          action.payload.snapshot.cursor.instance
        )
          state.lastCommand = null;
      },
      snapshotReceived(
        state,
        action: PayloadAction<{
          generation: number;
          snapshot: ApplicationSnapshot<C, O>;
        }>,
      ) {
        const connection = state.connection;
        if (
          connection.generation !== action.payload.generation ||
          connection.status !== 'connected' ||
          !connection.snapshot
        )
          return;
        const previous = connection.snapshot.cursor;
        const next = action.payload.snapshot.cursor;
        if (
          next.instance !== previous.instance ||
          BigInt(next.sequence) !== BigInt(previous.sequence) + 1n
        ) {
          connection.status = 'resync_required';
          connection.snapshot = null;
          connection.error = 'Observation needs a fresh snapshot.';
        } else
          connection.snapshot = action.payload.snapshot as Draft<
            ApplicationSnapshot<C, O>
          >;
      },
      resyncRequired(state, action: PayloadAction<number>) {
        if (state.connection.generation !== action.payload) return;
        state.connection.status = 'resync_required';
        state.connection.snapshot = null;
        state.connection.error = 'Observation needs a fresh snapshot.';
      },
      observationLost(
        state,
        action: PayloadAction<{ generation: number; message: string }>,
      ) {
        if (state.connection.generation !== action.payload.generation) return;
        state.connection.status = 'disconnected';
        state.connection.snapshot = null;
        state.connection.error = action.payload.message;
      },
      commandSubmitted(
        state,
        action: PayloadAction<{ id: OperationId; kind: CommandKind }>,
      ) {
        state.lastCommand = {
          id: action.payload.id,
          kind: action.payload.kind,
          phase: 'submitting',
          result: null,
          failure: null,
        };
      },
      commandAccepted(state, action: PayloadAction<OperationId>) {
        if (
          state.lastCommand &&
          sameOperation(state.lastCommand.id, action.payload)
        )
          state.lastCommand.phase = 'accepted';
      },
      commandFailed(
        state,
        action: PayloadAction<{ id: OperationId; failure: ClientFailure }>,
      ) {
        if (
          !state.lastCommand ||
          !sameOperation(state.lastCommand.id, action.payload.id)
        )
          return;
        state.lastCommand.phase =
          action.payload.failure.kind === 'outcome_unknown'
            ? 'outcome_unknown'
            : 'failed';
        state.lastCommand.failure = action.payload.failure;
      },
      operationReceived(
        state,
        action: PayloadAction<{ id: OperationId; status: OperationStatus }>,
      ) {
        if (
          !state.lastCommand ||
          !sameOperation(state.lastCommand.id, action.payload.id)
        )
          return;
        state.lastCommand.phase = 'accepted';
        state.lastCommand.failure = null;
        state.lastCommand.result = action.payload.status;
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
