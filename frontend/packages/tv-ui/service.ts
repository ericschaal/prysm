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
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
} from 'react';
import {
  problemOf,
  type LightingTarget,
  type LunaError,
  type ServiceEvent,
  type State,
  type TvService,
} from '@prysm/webos-platform';
import { same } from '@prysm/application-client';
import { modeRoute } from './modes';
import {
  completePreferences,
  sentPreferences,
  type SavedPreferences,
} from './preferences';

type Saved = SavedPreferences;
type Edits = Partial<Saved>;

/**
 * The subscription as the screen shows it. `missing` (set up the service) is
 * reached only after Remove setup, or when the bus cannot give the service
 * to a fresh call (`absent` while not `live`). Any other failure keeps what
 * is shown: the last state (`lost`, opened again after 3 s), or `connecting`
 * before any.
 */
export type Link =
  | { status: 'connecting' }
  | { status: 'live' | 'lost'; state: State }
  | { status: 'missing'; state: State | null };

export interface ServiceState {
  link: Link;
  /**
   * What the last save stored, as its reply says, and the preferences shown
   * when the reply came: an older push (showing `before`) may still arrive
   * after the reply. The first push showing anything else ends it: the
   * stored preferences, or a change made since.
   */
  pending: { stored: Saved; before: Saved | null } | null;
  changing: boolean;
  notice: string;
  error: boolean;
  /** The last failure as the service gave it, for Diagnostics only. */
  problem: string;
}

export type ServiceAction =
  | { type: 'event'; event: ServiceEvent }
  | { type: 'removed' }
  | { type: 'saved'; stored: Saved }
  | { type: 'changing'; value: boolean }
  | { type: 'notice'; text: string; error: boolean }
  | { type: 'problem'; text: string };

/** The last state received, whatever the subscription does now. */
export const lastState = (link: Link) =>
  link.status === 'connecting' ? null : link.state;

/**
 * The service is gone: the status card says so (its words go to
 * Diagnostics). The notice is cleared only as it goes: a failed setup's
 * notice stays through the resubscriptions that fail after it.
 */
const missing = (current: ServiceState): ServiceState =>
  current.link.status === 'missing'
    ? current
    : {
        ...current,
        link: { status: 'missing', state: lastState(current.link) },
        notice: '',
        error: false,
      };

export function serviceReducer(
  current: ServiceState,
  action: ServiceAction,
): ServiceState {
  switch (action.type) {
    case 'event': {
      const { event } = action;
      const { link } = current;
      // The bus answers a live subscription `absent` when the service ends,
      // and a fresh call starts a set-up service again: the next try
      // decides. A fresh call answered `absent` could not start it.
      if (!event.online)
        return {
          ...(event.error.kind === 'absent' && link.status !== 'live'
            ? missing(current)
            : {
                ...current,
                link:
                  link.status === 'live' ? { ...link, status: 'lost' } : link,
              }),
          problem: event.error.message,
        };
      const { state } = event;
      const saved = completePreferences(state.preferences);
      return {
        ...current,
        link: { status: 'live', state },
        pending:
          current.pending &&
          current.pending.before &&
          same(current.pending.before, saved)
            ? current.pending
            : null,
        ...(link.status === 'missing'
          ? { notice: 'Service connected.', error: false }
          : {}),
      };
    }
    case 'removed':
      return missing(current);
    case 'saved': {
      // The subscription may already show what was saved.
      const state = lastState(current.link);
      const before = state && completePreferences(state.preferences);
      return {
        ...current,
        pending: same(before, action.stored)
          ? null
          : { stored: action.stored, before },
      };
    }
    case 'changing':
      return { ...current, changing: action.value };
    case 'notice':
      return { ...current, notice: action.text, error: action.error };
    case 'problem':
      return { ...current, problem: action.text };
  }
}

export const initialService: ServiceState = {
  link: { status: 'connecting' },
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
  const [current, dispatch] = useReducer(serviceReducer, initialService);
  const [generation, setGeneration] = useState(0);
  /** The end of the queue of commands; it never rejects. */
  const queue = useRef<Promise<unknown>>(Promise.resolve());
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
    return service.watch((event) => dispatch({ type: 'event', event }));
  }, [service, active, generation]);
  const { link } = current;
  const state = lastState(link);
  const saved = completePreferences(state?.preferences ?? {});
  /** What the settings show: the saved values, or those being saved. */
  const draft: Saved = current.pending?.stored ?? saved;
  /**
   * The preferences a queued change applies to: the latest render's, or
   * what the change before it sent, until a render shows that.
   */
  const shown = useRef(draft);
  /** The latest render's link, for a command that ends after it. */
  const latest = useRef(link);
  useLayoutEffect(() => {
    shown.current = draft;
    latest.current = link;
  });
  const say = useCallback(
    (text: string, error = false) => dispatch({ type: 'notice', text, error }),
    [],
  );

  /**
   * Run `work` once the commands before it are done: the service runs one at
   * a time. Resolves `false` when it failed, which says `failed` in plain
   * words; the service's own message goes to Diagnostics. `begun` is said
   * when it starts.
   */
  function run(
    work: () => Promise<unknown>,
    done: string | undefined,
    failed: string,
    begun?: string,
  ): Promise<boolean> {
    dispatch({ type: 'changing', value: true });
    const result: Promise<boolean> = queue.current.then(async () => {
      try {
        // The app closed meanwhile: nothing more is sent.
        if (!mounted.current) return false;
        if (begun !== undefined) say(begun);
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
        if (mounted.current && queue.current === result)
          dispatch({ type: 'changing', value: false });
      }
    });
    queue.current = result;
    return result;
  }

  /**
   * Answer what the service asks with the intent it keeps: slim, capture
   * on; full, the target again (off first while the worker runs, so it is
   * applied anew). Nothing is sent when nothing is asked, or for a target
   * newer than this app.
   */
  async function answer() {
    const now = latest.current;
    if (now.status !== 'live' && now.status !== 'lost') return;
    const asked = now.state;
    if (!problemOf(asked)) return;
    if (asked.flavor === 'slim') return void (await service.setEnabled(true));
    if (asked.target === 'unknown') return;
    if (modeRoute(now).via === 'portal') await service.setTarget('off');
    await service.setTarget(asked.target);
  }

  async function sendPreferences(patch: Edits) {
    const sent = { ...shown.current, ...patch };
    const reply = await service.setPreferences(sentPreferences(sent));
    const stored = completePreferences(reply.preferences);
    shown.current = stored;
    if (mounted.current) dispatch({ type: 'saved', stored });
  }

  return {
    ...current,
    /** The last state received, also kept while the service is missing. */
    state,
    saved,
    draft,
    say,
    /** Apply `patch` to the preferences now; only a failure is reported (an older notice is cleared). */
    set: (patch: Edits) => run(() => sendPreferences(patch), '', NOT_SAVED),
    /** Slim package: capture on or off. */
    enable: (enabled: boolean) =>
      run(
        () => service.setEnabled(enabled),
        '',
        enabled ? 'Couldn’t start.' : 'Couldn’t stop.',
      ),
    /** Full package: what the service asks, tried again ({@link answer}). */
    retry: () => run(answer, '', 'Couldn’t start.'),
    /** Full package: a lighting mode, where it goes to the service (see `modeRoute`). */
    choose: (target: LightingTarget | 'remembered') =>
      run(
        () => service.setTarget(target),
        '',
        target === 'off' ? 'Couldn’t switch off.' : 'Couldn’t start.',
      ),
    setup: () =>
      run(
        async () => {
          await service.setup();
          setGeneration((count) => count + 1);
          await answer();
        },
        'Service setup complete.',
        'Setup didn’t finish.',
        'Setting up the service. This may take a moment.',
      ),
    uninstall: () =>
      run(
        async () => {
          await service.uninstall();
          if (mounted.current) dispatch({ type: 'removed' });
        },
        'Service setup removed. Your settings are kept.',
        'Couldn’t remove the setup.',
      ),
  };
}

export type TvServiceView = ReturnType<typeof useTvService>;
