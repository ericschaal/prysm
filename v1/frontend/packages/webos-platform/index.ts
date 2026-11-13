/**
 * The TV supervisor's Luna API (`luna://io.prysm.tv.service`, both packages)
 * over the webOS service bridge. Values are the generated `luna` contract;
 * replies are checked before use because the bridge hands over plain text.
 * See docs/plans/prysm-v1-webos-supervisor.md §6.
 */
import type {
  Flavor,
  LightingTarget,
  Phase,
  Power,
  Preferences,
  SlimEncoding,
  State,
  Video,
} from '@prysm/contracts/luna';

export type {
  Flavor,
  LightingTarget,
  Phase,
  Power,
  Preferences,
  SlimEncoding,
  State,
  Video,
};

export const SERVICE_URI = 'luna://io.prysm.tv.service';
/** Installed by the IPK; run as root through the Homebrew Channel. */
export const SETUP_SCRIPT =
  '/media/developer/apps/usr/palm/services/io.prysm.tv.service/setup.sh';
export const HOMEBREW_EXEC = 'luna://org.webosbrew.hbchannel.service/exec';
/** A call or the first subscription reply that takes longer has failed. */
export const CALL_TIMEOUT_MS = 30_000;
/** Delay before a lost subscription is opened again. */
export const RESUBSCRIBE_MS = 3_000;

/**
 * - `unavailable`: no bridge (not on a TV) or the service is not registered.
 * - `timeout`: no reply within {@link CALL_TIMEOUT_MS}.
 * - `refused`: the service answered `returnValue: false` (its `errorText`).
 * - `invalid_reply`: the reply is not what the contract says.
 */
export type LunaFailureKind =
  'unavailable' | 'timeout' | 'refused' | 'invalid_reply';

export class LunaError extends Error {
  constructor(
    readonly kind: LunaFailureKind,
    message: string,
  ) {
    super(message);
  }
}

/** The page's view of `PalmServiceBridge` / `WebOSServiceBridge`. */
export interface ServiceBridge {
  onservicecallback: ((text: string) => void) | null;
  call(uri: string, payload: string): void;
  cancel(): void;
}
export type BridgeFactory = () => ServiceBridge;

/** The TV's bridge, or `null` when the page does not run in a webOS app. */
export function webosBridge(): BridgeFactory | null {
  const scope = window as unknown as Record<
    string,
    (new () => ServiceBridge) | undefined
  >;
  const Bridge = scope.PalmServiceBridge ?? scope.WebOSServiceBridge;
  return Bridge ? () => new Bridge() : null;
}

/** One state from the subscription, or why it is not available. */
export type ServiceEvent =
  { online: true; state: State } | { online: false; error: LunaError };

export interface TvService {
  /**
   * Subscribe to `getState`: every state, and a failure whenever the
   * subscription is lost; it is opened again after {@link RESUBSCRIBE_MS}.
   * Returns a function that ends it.
   */
  watch(listener: (event: ServiceEvent) => void): () => void;
  /** Store the complete preferences; resolves with the new state. */
  setPreferences(preferences: Preferences): Promise<State>;
  setEnabled(enabled: boolean): Promise<State>;
  /** Stop the worker, remove the setup and end the service; settings stay. */
  uninstall(): Promise<void>;
  /** Run `setup.sh` as root through the Homebrew Channel (set up or repair). */
  setup(): Promise<void>;
  /** Cancel every call still waiting for a reply. */
  cancelAll(): void;
}

/** Keyed by every value of the generated union, so the compiler keeps them in step. */
const flavors: Record<Flavor, true> = { full: true, slim: true };
const powers: Record<Power, true> = {
  unknown: true,
  active: true,
  screen_saver: true,
  asleep: true,
};
const videos: Record<Video, true> = {
  unknown: true,
  unavailable: true,
  available: true,
};
const phases: Record<Phase, true> = {
  stopped: true,
  running: true,
  stopping: true,
  retrying: true,
};
const targets: Record<LightingTarget, true> = {
  off: true,
  processed: true,
  device_native: true,
};
const oneOf = (values: object, value: unknown) =>
  typeof value === 'string' && Object.keys(values).indexOf(value) >= 0;
const optionalText = (value: unknown) =>
  value === undefined || value === null || typeof value === 'string';

export function isState(value: unknown): value is State {
  const state = value as Partial<Record<keyof State, unknown>> | null;
  return (
    !!state &&
    typeof state === 'object' &&
    oneOf(flavors, state.flavor) &&
    oneOf(powers, state.power) &&
    oneOf(videos, state.video) &&
    oneOf(phases, state.worker) &&
    typeof state.enabled === 'boolean' &&
    typeof state.capturing === 'boolean' &&
    !!state.preferences &&
    typeof state.preferences === 'object' &&
    Array.isArray(state.log) &&
    (state.log as unknown[]).every((line) => typeof line === 'string') &&
    optionalText(state.failure) &&
    optionalText(state.portal) &&
    (state.lighting === undefined ||
      state.lighting === null ||
      oneOf(targets, state.lighting))
  );
}

type Reply = Record<string, unknown>;

function decode(text: string): Reply {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw new LunaError(
      'invalid_reply',
      'The TV service sent an unreadable reply.',
    );
  }
  if (!value || typeof value !== 'object')
    throw new LunaError(
      'invalid_reply',
      'The TV service sent an unreadable reply.',
    );
  const reply = value as Reply;
  if (reply.returnValue !== true) {
    const text =
      typeof reply.errorText === 'string' && reply.errorText
        ? reply.errorText
        : 'The TV service could not complete this action.';
    // LS2 itself answers this way when the service is not registered.
    const missing =
      /not found|unknown service|no such service|service does not exist/i.test(
        text,
      );
    throw new LunaError(missing ? 'unavailable' : 'refused', text);
  }
  return reply;
}

function stateOf(reply: Reply): State {
  if (!isState(reply))
    throw new LunaError(
      'invalid_reply',
      'The TV service sent an unexpected state.',
    );
  const state = { ...reply } as Reply;
  delete state.returnValue;
  delete state.subscribed;
  return state as unknown as State;
}

const notOnTv = () =>
  new LunaError(
    'unavailable',
    'Open Prysm on your LG TV to reach its service.',
  );

/** The Luna client. `bridge` is `null` off the TV: every call fails `unavailable`. */
export function lunaTvService(
  bridge: BridgeFactory | null = webosBridge(),
): TvService {
  const pending = new Set<() => void>();

  /** One call; `onReply` returns `true` when the call is complete. */
  function open(
    uri: string,
    payload: object,
    onReply: (text: string) => boolean,
    onFailure: (error: LunaError) => void,
    timeoutMs: number | null,
  ): () => void {
    if (!bridge) {
      onFailure(notOnTv());
      return () => {};
    }
    let channel: ServiceBridge;
    try {
      channel = bridge();
    } catch (error) {
      onFailure(new LunaError('unavailable', String(error)));
      return () => {};
    }
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const release = () => {
      if (!active) return false;
      active = false;
      clearTimeout(timer);
      pending.delete(cancel);
      return true;
    };
    function cancel() {
      if (!release()) return;
      // Cancelling may call back synchronously; it is ignored once released.
      channel.cancel();
    }
    const fail = (error: LunaError) => {
      if (!active) return;
      cancel();
      onFailure(error);
    };
    if (timeoutMs !== null)
      timer = setTimeout(
        () => fail(new LunaError('timeout', 'The TV service did not respond.')),
        timeoutMs,
      );
    pending.add(cancel);
    channel.onservicecallback = (text) => {
      if (!active) return;
      clearTimeout(timer);
      let done: boolean;
      try {
        done = onReply(text);
      } catch (error) {
        fail(
          error instanceof LunaError
            ? error
            : new LunaError('invalid_reply', String(error)),
        );
        return;
      }
      if (done) cancel();
    };
    try {
      channel.call(uri, JSON.stringify(payload));
    } catch (error) {
      fail(
        new LunaError(
          'unavailable',
          error instanceof Error ? error.message : String(error),
        ),
      );
    }
    return cancel;
  }

  function request<T>(
    uri: string,
    payload: object,
    parse: (reply: Reply) => T,
  ) {
    return new Promise<T>((resolve, reject) => {
      open(
        uri,
        payload,
        (text) => {
          resolve(parse(decode(text)));
          return true;
        },
        reject,
        CALL_TIMEOUT_MS,
      );
    });
  }
  const method = (name: string) => `${SERVICE_URI}/${name}`;

  return {
    watch(listener) {
      let stopped = false;
      let close = () => {};
      let retry: ReturnType<typeof setTimeout> | undefined;
      function failed(error: LunaError) {
        if (stopped) return;
        listener({ online: false, error });
        retry = setTimeout(subscribe, RESUBSCRIBE_MS);
      }
      function subscribe() {
        let first = true;
        close = open(
          method('getState'),
          { subscribe: true },
          (text) => {
            const reply = decode(text);
            if (first && reply.subscribed !== true)
              throw new LunaError(
                'refused',
                'The TV service did not accept a subscription.',
              );
            first = false;
            if (!stopped) listener({ online: true, state: stateOf(reply) });
            return false;
          },
          failed,
          CALL_TIMEOUT_MS,
        );
      }
      subscribe();
      return () => {
        stopped = true;
        clearTimeout(retry);
        close();
      };
    },
    setPreferences: (preferences) =>
      request(method('setPreferences'), preferences, stateOf),
    setEnabled: (enabled) =>
      request(method('setEnabled'), { enabled }, stateOf),
    uninstall: () => request(method('uninstall'), {}, () => undefined),
    setup: () =>
      new Promise<void>((resolve, reject) => {
        open(
          HOMEBREW_EXEC,
          { command: SETUP_SCRIPT },
          (text) => {
            let reply: Reply;
            try {
              reply = JSON.parse(text) as Reply;
            } catch {
              throw new LunaError(
                'invalid_reply',
                'Homebrew Channel sent an unreadable reply.',
              );
            }
            if (reply?.returnValue === true) {
              resolve();
              return true;
            }
            const detail = [reply?.errorText, reply?.stderrString].find(
              (value) => typeof value === 'string' && value,
            ) as string | undefined;
            const missing = /not found|unknown service|no such service/i.test(
              detail ?? '',
            );
            throw new LunaError(
              missing ? 'unavailable' : 'refused',
              missing
                ? 'Setup needs the Homebrew Channel with root access.'
                : (detail ?? 'Setup failed.'),
            );
          },
          reject,
          CALL_TIMEOUT_MS,
        );
      }),
    cancelAll() {
      for (const cancel of Array.from(pending)) cancel();
    },
  };
}
