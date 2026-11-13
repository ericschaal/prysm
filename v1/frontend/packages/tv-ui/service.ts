/**
 * The TV service as the UI sees it: the subscribed state, a draft of the
 * preferences and the commands. The subscription is the only source of
 * state; command replies only confirm (they may overtake an older push).
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
type Key = keyof Saved;

interface ServiceState {
  online: boolean | null;
  state: State | null;
  /** When `state` arrived (`Date.now()`), for countdowns it carries. */
  stateAt: number;
  /** Unsaved values, and the saved value each edit started from. */
  edits: Edits;
  bases: Edits;
  /** Sent with the last successful save, until the subscription shows it. */
  pending: Saved | null;
  changing: boolean;
  notice: string;
  error: boolean;
}

type Action =
  | { type: 'event'; event: ServiceEvent; at: number }
  | { type: 'edit'; patch: Edits }
  | { type: 'saved'; sent: Saved }
  | { type: 'settled' }
  | { type: 'discard' }
  | { type: 'changing'; value: boolean }
  | { type: 'notice'; text: string; error: boolean };

const keys = (edits: Edits) => Object.keys(edits) as Key[];

/** The notice for a pushed state, when it needs one. */
function eventNotice(replaced: boolean, reconnected: boolean) {
  if (replaced)
    return {
      notice:
        'Settings were changed elsewhere; your unsaved change was replaced.',
      error: true,
    };
  if (reconnected) return { notice: 'Service connected.', error: false };
  return {};
}

function reducer(current: ServiceState, action: Action): ServiceState {
  switch (action.type) {
    case 'event': {
      const { event } = action;
      if (!event.online)
        return {
          ...current,
          online: false,
          notice: event.error.message,
          error: true,
        };
      const saved = completePreferences(event.state.preferences);
      const edits: Edits = { ...current.edits };
      const bases: Edits = { ...current.bases };
      let replaced = false;
      for (const key of keys(edits)) {
        if (!same(bases[key], saved[key])) replaced = true;
        if (!same(bases[key], saved[key]) || same(edits[key], saved[key])) {
          delete edits[key];
          delete bases[key];
        }
      }
      const reconnected = current.online === false;
      return {
        ...current,
        online: true,
        state: event.state,
        stateAt: action.at,
        edits,
        bases,
        pending:
          current.pending && !same(current.pending, saved)
            ? current.pending
            : null,
        ...eventNotice(replaced, reconnected),
      };
    }
    case 'edit': {
      const saved = completePreferences(current.state?.preferences ?? {});
      const edits = { ...current.edits };
      const bases = { ...current.bases };
      for (const key of keys(action.patch)) {
        if (!(key in bases)) (bases as Record<Key, unknown>)[key] = saved[key];
        (edits as Record<Key, unknown>)[key] = action.patch[key];
        if (same(edits[key], saved[key])) {
          delete edits[key];
          delete bases[key];
        }
      }
      return { ...current, edits, bases };
    }
    case 'saved': {
      // The subscription may already show what was saved.
      const shown =
        current.state &&
        same(completePreferences(current.state.preferences), action.sent);
      return {
        ...current,
        edits: {},
        bases: {},
        pending: shown ? null : action.sent,
      };
    }
    case 'settled':
      return { ...current, pending: null };
    case 'discard':
      return { ...current, edits: {}, bases: {} };
    case 'changing':
      return { ...current, changing: action.value };
    case 'notice':
      return { ...current, notice: action.text, error: action.error };
  }
}

const initial: ServiceState = {
  online: null,
  state: null,
  stateAt: 0,
  edits: {},
  bases: {},
  pending: null,
  changing: false,
  notice: '',
  error: false,
};

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
  const draft: Saved = { ...(current.pending ?? saved), ...current.edits };
  const dirty = keys(current.edits).length > 0;
  const say = useCallback(
    (text: string, error = false) => dispatch({ type: 'notice', text, error }),
    [],
  );
  const edit = useCallback(
    (patch: Edits) => dispatch({ type: 'edit', patch }),
    [],
  );

  /** Run one command at a time; resolves `false` when it did not run or failed. */
  async function run(work: () => Promise<unknown>, done?: string) {
    if (busy.current) return false;
    busy.current = true;
    dispatch({ type: 'changing', value: true });
    try {
      await work();
      if (done !== undefined && mounted.current) say(done);
      return true;
    } catch (error) {
      if (mounted.current) say(messageOf(error), true);
      return false;
    } finally {
      busy.current = false;
      if (mounted.current) dispatch({ type: 'changing', value: false });
    }
  }

  async function sendPreferences(patch: Edits = {}) {
    const sent = { ...draft, ...patch };
    await service.setPreferences(sent);
    if (mounted.current) dispatch({ type: 'saved', sent });
  }

  return {
    ...current,
    saved,
    draft,
    dirty,
    syncing: current.pending !== null,
    edit,
    discard: () => dispatch({ type: 'discard' }),
    say,
    save: () => run(() => sendPreferences(), 'Settings saved.'),
    /** Save the draft with `patch` applied, without a separate Save. */
    saveWith: (patch: Edits, done: string) =>
      run(() => sendPreferences(patch), done),
    enable: (enabled: boolean) => run(() => service.setEnabled(enabled), ''),
    saveAndEnable: () =>
      run(async () => {
        await sendPreferences();
        await service.setEnabled(true);
      }, 'Settings saved.'),
    setup: () => {
      say('Setting up the service. This may take a moment.');
      return run(async () => {
        await service.setup();
        setGeneration((count) => count + 1);
      }, 'Service setup complete.');
    },
    uninstall: () =>
      run(
        () => service.uninstall(),
        'Service setup removed. Your settings are kept.',
      ),
  };
}

export type TvServiceView = ReturnType<typeof useTvService>;
