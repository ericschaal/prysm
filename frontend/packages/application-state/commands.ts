import type { ThunkAction, UnknownAction } from '@reduxjs/toolkit';
import {
  ApplicationError,
  failureOf,
  type ApplicationClient,
  type CalibrationPattern,
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
        dispatch(actions.commandFailed({ id, failure: failureOf(error) }));
        throw error;
      }
    };
  }
  /**
   * Replace one adapter section of the observed settings, keeping the rest.
   * `expected` is the revision the edit started from; a newer observed
   * revision is a conflict the user resolves, never a silent overwrite.
   * While not connected the observed settings may be stale, so nothing is
   * merged into them.
   */
  function replaceSection(
    kind: CommandKind,
    expected: number,
    change: (settings: Settings<C, O>) => Settings<C, O>,
  ): ApplicationThunk<C, O, Promise<number>> {
    return submit(kind, (client, state) => {
      const { status, snapshot } = state.connection;
      const live = status === 'connected' ? snapshot : null;
      if (!live || live.revision !== expected)
        return Promise.reject(
          new ApplicationError({
            kind: 'stale_revision',
            message: 'Settings changed; review them before saving again.',
            // Disconnected, the revision last seen: saving waits for the next.
            current: snapshot?.revision ?? expected,
          }),
        );
      return client.replaceSettings(
        expected,
        // Settings are JSON; `structuredClone` needs Chromium 98, newer than many TVs.
        change(JSON.parse(JSON.stringify(live.settings)) as Settings<C, O>),
      );
    });
  }
  return {
    setTarget: (target: LightingTarget) =>
      submit('setTarget', (client) => client.setTarget(target)),
    setCalibration: (calibration: CalibrationPattern | null) =>
      submit('setCalibration', (client) => client.setCalibration(calibration)),
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
    /** Capture and output together, as one save (the portal's guided setup). */
    configureSetup: (expected: number, capture: C, output: O) =>
      replaceSection('configureSetup', expected, (settings) => ({
        ...settings,
        capture,
        output,
      })),
    /**
     * Light with `processing` unsaved, as a slider moves. Not a tracked
     * command: it changes no state and its failures are the caller's.
     */
    tryProcessing:
      (
        processing: Settings<C, O>['processing'],
      ): ApplicationThunk<C, O, Promise<void>> =>
      (_dispatch, _getState, client) =>
        client.tryProcessing(processing),
    configureProcessing: (
      expected: number,
      processing: Settings<C, O>['processing'],
    ) =>
      replaceSection('configureProcessing', expected, (settings) => ({
        ...settings,
        processing,
      })),
  };
}
