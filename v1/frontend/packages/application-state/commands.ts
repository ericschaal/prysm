import type { ThunkAction, UnknownAction } from '@reduxjs/toolkit';
import {
  ApplicationError,
  newRequestId,
  type Accepted,
  type ApplicationClient,
  type ClientFailure,
  type OperationId,
  type OperationStatus,
  type TargetRequest,
} from '@prysm/application-client';
import type {
  ApplicationActions,
  ApplicationState,
  CommandKind,
} from './slice';

export type ApplicationThunk<C, O, Result> = ThunkAction<
  Result,
  ApplicationState<C, O>,
  ApplicationClient<C, O>,
  UnknownAction
>;

export function requestId<C, O>(state: ApplicationState<C, O>) {
  const { connection } = state;
  if (connection.status !== 'connected' || !connection.snapshot) {
    throw new ApplicationError({
      kind: 'unavailable',
      message: 'Application is not synchronized.',
    });
  }
  return newRequestId(connection.snapshot.cursor.instance);
}

export function createCommands<C, O>(actions: ApplicationActions<C, O>) {
  function submit(
    kind: CommandKind,
    send: (
      client: ApplicationClient<C, O>,
      id: OperationId,
    ) => Promise<Accepted>,
  ): ApplicationThunk<C, O, Promise<Accepted>> {
    return async (dispatch, getState, client) => {
      const id = requestId(getState());
      dispatch(actions.commandSubmitted({ id, kind }));
      try {
        const accepted = await send(client, id);
        dispatch(actions.commandAccepted(accepted.operation));
        return accepted;
      } catch (error) {
        const failure: ClientFailure =
          error instanceof ApplicationError
            ? error.detail
            : { kind: 'unavailable', message: String(error) };
        dispatch(actions.commandFailed({ id, failure }));
        throw error;
      }
    };
  }
  return {
    configureCapture: (expected: string, settings: C) =>
      submit('configureCapture', (client, id) =>
        client.configureCapture({ id, expected, settings }),
      ),
    configureOutput: (expected: string, settings: O) =>
      submit('configureOutput', (client, id) =>
        client.configureOutput({ id, expected, settings }),
      ),
    setTarget: (target: TargetRequest['target']) =>
      submit('setTarget', (client, id) => client.setTarget({ id, target })),
    checkOperation:
      (id: OperationId): ApplicationThunk<C, O, Promise<OperationStatus>> =>
      async (dispatch, _getState, client) => {
        const status = await client.operation(id);
        dispatch(actions.operationReceived({ id, status }));
        return status;
      },
  };
}
