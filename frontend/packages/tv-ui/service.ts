/**
 * The TV service as the UI sees it: the subscribed state, the preferences as
 * shown and the commands. The subscription is the only source of state;
 * command replies only confirm (they may overtake an older push).
 *
 * A preference applies as soon as it is chosen (OK on a switch, or a choice
 * confirmed in its dialog), like the portal's live controls: arrows only
 * move focus, so nothing changes without OK, and there is no draft to save
 * or lose. A failed change leaves the saved value shown and says so.
 */
import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import type {
  LunaError,
  ServiceEvent,
  State,
  TvService,
} from '@prysm/webos-platform';
import { same } from '@prysm/application-client';
import { completePreferences, type SavedPreferences } from './model';

type Saved = SavedPreferences;
type Edits = Partial<Saved>;

interface ServiceState {
  online: boolean | null;
  state: State | null;
  /** When `state` arrived (`Date.now()`), for countdowns it carries. */
  stateAt: number;
  /** Sent with the last successful save, until the subscription shows it. */
  pending: Saved | null;
  changing: boolean;
  notice: string;
  error: boolean;
  /** The last failure as the service gave it, for Diagnostics only. */
  problem: string;
}

type Action =
  | { type: 'event'; event: ServiceEvent; at: number }
  | { type: 'saved'; sent: Saved }
  | { type: 'settled' }
  | { type: 'changing'; value: boolean }
  | { type: 'notice'; text: string; error: boolean }
  | { type: 'problem'; text: string };

function reducer(current: ServiceState, action: Action): ServiceState {
  switch (action.type) {
    case 'event': {
      const { event } = action;
      // The status card says it; the service's words go to Diagnostics.
      if (!event.online)
        return {
          ...current,
          online: false,
          notice: '',
          error: false,
          problem: event.error.message,
        };
      const saved = completePreferences(event.state.preferences);
      return {
        ...current,
        online: true,
        state: event.state,
        stateAt: action.at,
        pending:
          current.pending && !same(current.pending, saved)
            ? current.pending
            : null,
        ...(current.online === false
          ? { notice: 'Service connected.', error: false }
          : {}),
      };
    }
    case 'saved': {
      // The subscription may already show what was saved.
      const shown =
        current.state &&
        same(completePreferences(current.state.preferences), action.sent);
      return { ...current, pending: shown ? null : action.sent };
    }
    case 'settled':
      return { ...current, pending: null };
    case 'changing':
      return { ...current, changing: action.value };
    case 'notice':
      return { ...current, notice: action.text, error: action.error };
    case 'problem':
      return { ...current, problem: action.text };
  }
}

const initial: ServiceState = {
  online: null,
  state: null,
  stateAt: 0,
  pending: null,
  changing: false,
  notice: '',
  error: false,
  problem: '',
};

const NOT_SAVED = 'Couldn’t save your settings.';
const DETAILS = 'Service options › Diagnostics has the details.';

const messageOf = (error: unknown) =>
  (error as LunaError | Error | undefined)?.message ?? String(error);

export function useTvService(service: TvService, active: boolean) {
  const [current, dispatch] = useReducer(reducer, initial);
  const [generation, setGeneration] = useState(0);
  const busy = useRef(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      service.cancelAll();
    };
  }, [service]);
  useEffect(() => {
    if (!active) return;
    return service.watch((event) =>
      dispatch({ type: 'event', event, at: Date.now() }),
    );
  }, [service, active, generation]);
  useEffect(() => {
    if (!current.pending) return;
    // The subscription normally confirms within a moment; never wait forever.
    const timer = setTimeout(() => dispatch({ type: 'settled' }), 5000);
    return () => clearTimeout(timer);
  }, [current.pending]);

  const saved = completePreferences(current.state?.preferences ?? {});
  /** What the settings show: the saved values, or those being saved. */
  const draft: Saved = current.pending ?? saved;
  const say = useCallback(
    (text: string, error = false) => dispatch({ type: 'notice', text, error }),
    [],
  );

  /**
   * Run one command at a time; resolves `false` when it did not run or
   * failed. A failure says `failed` in plain words; the service's own
   * message goes to Diagnostics.
   */
  async function run(
    work: () => Promise<unknown>,
    done: string | undefined,
    failed: string,
  ) {
    if (busy.current) return false;
    busy.current = true;
    dispatch({ type: 'changing', value: true });
    try {
      await work();
      if (done !== undefined && mounted.current) say(done);
      return true;
    } catch (error) {
      if (mounted.current) {
        dispatch({ type: 'problem', text: messageOf(error) });
        say(`${failed} ${DETAILS}`, true);
      }
      return false;
    } finally {
      busy.current = false;
      if (mounted.current) dispatch({ type: 'changing', value: false });
    }
  }

  async function sendPreferences(patch: Edits) {
    const sent = { ...draft, ...patch };
    await service.setPreferences(sent);
    if (mounted.current) dispatch({ type: 'saved', sent });
  }

  return {
    ...current,
    saved,
    draft,
    say,
    /** Apply `patch` to the preferences now; only a failure is reported (an older notice is cleared). */
    set: (patch: Edits) => run(() => sendPreferences(patch), '', NOT_SAVED),
    enable: (enabled: boolean) =>
      run(
        () => service.setEnabled(enabled),
        '',
        enabled ? 'Couldn’t start.' : 'Couldn’t stop.',
      ),
    /** Full package: off and on again, as after a failure. */
    retry: () =>
      run(
        async () => {
          await service.setEnabled(false);
          await service.setEnabled(true);
        },
        '',
        'Couldn’t start.',
      ),
    setup: () => {
      say('Setting up the service. This may take a moment.');
      return run(
        async () => {
          await service.setup();
          setGeneration((count) => count + 1);
        },
        'Service setup complete.',
        'Setup didn’t finish.',
      );
    },
    uninstall: () =>
      run(
        () => service.uninstall(),
        'Service setup removed. Your settings are kept.',
        'Couldn’t remove the setup.',
      ),
  };
}

export type TvServiceView = ReturnType<typeof useTvService>;
