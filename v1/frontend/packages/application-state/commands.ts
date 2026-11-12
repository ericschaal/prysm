import type { ThunkAction, UnknownAction } from '@reduxjs/toolkit';
import {
  ApplicationError,
  type ApplicationClient,
  type Failure,
  type LightingTarget,
  type Settings,
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

/** Typed command thunks. Each resolves with the runtime's own result. */
export function createCommands<C, O>(actions: ApplicationActions<C, O>) {
  let sequence = 0;
  function submit<T>(
    kind: CommandKind,
    send: (
      client: ApplicationClient<C, O>,
      state: ApplicationState<C, O>,
    ) => Promise<T>,
  ): ApplicationThunk<C, O, Promise<T>> {
    return async (dispatch, getState, client) => {
      const id = ++sequence;
      dispatch(actions.commandSubmitted({ id, kind }));
      try {
        const result = await send(client, getState());
        dispatch(actions.commandSucceeded(id));
        return result;
      } catch (error) {
        const failure: Failure =
          error instanceof ApplicationError
            ? error.failure
            : {
                kind: 'internal',
                message: error instanceof Error ? error.message : String(error),
              };
        dispatch(actions.commandFailed({ id, failure }));
        throw error;
      }
    };
  }
  /**
   * Replace one adapter section of the observed settings, keeping the rest.
   * `expected` is the revision the edit started from; a newer observed
   * revision is a conflict the user resolves, never a silent overwrite.
   */
  function replaceSection(
    kind: CommandKind,
    expected: number,
    change: (settings: Settings<C, O>) => Settings<C, O>,
  ): ApplicationThunk<C, O, Promise<number>> {
    return submit(kind, (client, state) => {
      const snapshot = state.connection.snapshot;
      if (!snapshot || snapshot.revision !== expected)
        return Promise.reject(
          new ApplicationError({
            kind: 'stale_revision',
            message: 'Settings changed; review them before saving again.',
            ...(snapshot ? { current: snapshot.revision } : {}),
          }),
        );
      return client.replaceSettings(
        expected,
        change(structuredClone(snapshot.settings)),
      );
    });
  }
  return {
    setTarget: (target: LightingTarget) =>
      submit('setTarget', (client) => client.setTarget(target)),
    configureCapture: (expected: number, capture: C) =>
      replaceSection('configureCapture', expected, (settings) => ({
        ...settings,
        capture,
      })),
    configureOutput: (expected: number, output: O) =>
      replaceSection('configureOutput', expected, (settings) => ({
        ...settings,
        output,
      })),
  };
}
