/**
 * The TV supervisor's Luna API (`luna://<APP_ID>.service`, both packages)
 * over the webOS service bridge. Values are the generated `luna` contract;
 * replies are checked before use because the bridge hands over plain text.
 * See docs/plans/prysm-v1-webos-supervisor.md §6.
 */
import type {
  FailureKind,
  FullWorker,
  LightingTarget,
  Package,
  Power,
  Preferences,
  SlimEncoding,
  SlimWorker,
  State,
  Video,
  WorkerFailure,
} from '@prysm/contracts/luna';

export type {
  FailureKind,
  FullWorker,
  LightingTarget,
  Power,
  Preferences,
  SlimEncoding,
  SlimWorker,
  State,
  Video,
  WorkerFailure,
};

/** The installed package, the tag of {@link State}. */
export type Flavor = State['flavor'];
/** Either package's worker process, tagged by `phase`. */
export type Worker = State['worker'];
/** Slim: where the running worker's stream to Prysm stands, tagged by `state`. */
export type SlimLink = Extract<SlimWorker, { phase: 'running' }>['link'];

/** Lighting is not off (full), or frames should flow (slim). */
export function isEnabled(state: State): boolean {
  return state.flavor === 'full' ? state.lighting !== 'off' : state.enabled;
}

/** Frames flow from a running worker (both packages report it). */
export function isCapturing(state: State): boolean {
  const worker = state.worker;
  return worker.phase === 'running' && worker.capturing;
}

/** The running worker's frame rate, 0 while it waits for frames; `null` while none is measured. */
export function captureFps(state: State): number | null {
  if (state.flavor === 'slim') {
    const link = slimLink(state);
    return link?.state === 'sending' ? link.fps : null;
  }
  const worker = state.worker;
  return worker.phase === 'running' ? (worker.fps ?? null) : null;
}

/** Full: the portal's LAN address while the worker runs or stops; else `null`. */
export function fullPortal(state: State): string | null {
  if (state.flavor !== 'full') return null;
  const worker = state.worker;
  return worker.phase === 'running' || worker.phase === 'stopping'
    ? (worker.portal ?? null)
    : null;
}

/** Slim: the running worker's stream to Prysm; else `null`. */
function slimLink(state: State): SlimLink | null {
  if (state.flavor !== 'slim') return null;
  const worker = state.worker;
  return worker.phase === 'running' ? worker.link : null;
}

/** Slim: the encoding the running worker sends in; else `null`. */
export function slimEncoding(state: State): SlimEncoding | null {
  const link = slimLink(state);
  return link?.state === 'sending' ? link.encoding : null;
}

/** Slim: the receiver paused the running worker's stream (its lights are off). */
export function isPaused(state: State): boolean {
  return slimLink(state)?.state === 'paused';
}

/** The TV app of both packages; its service is `<APP_ID>.service`. */
export const APP_ID = 'io.github.ericschaal.prysm';
export const SERVICE_URI = `luna://${APP_ID}.service`;
/** Installed by the IPK; run as root through the Homebrew Channel. */
export const SETUP_SCRIPT = `/media/developer/apps/usr/palm/services/${APP_ID}.service/setup.sh`;
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
const failureKinds: Record<FailureKind, true> = {
  encoder: true,
  receiver: true,
  version: true,
  replaced: true,
  other: true,
};
const targets: Record<LightingTarget, true> = {
  off: true,
  processed: true,
  device_native: true,
};
/** `raw` if it is one of `values`, else `fallback`: a value newer than this app. */
function known<T extends string>(
  values: Record<T, true>,
  raw: string,
  fallback: T,
): T {
  return Object.keys(values).indexOf(raw) >= 0 ? (raw as T) : fallback;
}
// An absent value is omitted, never `null`.
const optionalText = (value: unknown) =>
  value === undefined || typeof value === 'string';
const optionalNumber = (value: unknown) =>
  value === undefined || typeof value === 'number';

type Fields = Partial<Record<string, unknown>>;
const isObject = (value: unknown): value is Fields =>
  !!value && typeof value === 'object';

/** A failure; an unknown kind is `other`. `null` when malformed. */
function decodeFailure(value: unknown): WorkerFailure | null {
  if (
    !isObject(value) ||
    typeof value.kind !== 'string' ||
    typeof value.message !== 'string'
  )
    return null;
  return {
    kind: known(failureKinds, value.kind, 'other'),
    message: value.message,
  };
}

/**
 * The phases both packages share: stopped, and retrying with its seconds; a
 * phase newer than this app is read as stopped. `null` when malformed.
 */
function decodeCommonPhase(
  worker: Fields,
): { phase: 'stopped' } | { phase: 'retrying'; seconds: number } | null {
  if (worker.phase !== 'retrying') return { phase: 'stopped' };
  return typeof worker.seconds === 'number'
    ? { phase: 'retrying', seconds: worker.seconds }
    : null;
}

function decodeFullWorker(value: unknown): FullWorker | null {
  if (!isObject(value) || typeof value.phase !== 'string') return null;
  switch (value.phase) {
    case 'running':
      return typeof value.capturing === 'boolean' &&
        optionalNumber(value.fps) &&
        optionalText(value.portal)
        ? (value as unknown as FullWorker)
        : null;
    case 'stopping':
      return optionalText(value.portal)
        ? (value as unknown as FullWorker)
        : null;
    default:
      return decodeCommonPhase(value);
  }
}

/**
 * Slim: where the stream stands; a state newer than this app is read as
 * starting. An encoding newer than this app is kept (it is shown as sent).
 */
function decodeSlimLink(value: unknown): SlimLink | null {
  if (!isObject(value) || typeof value.state !== 'string') return null;
  switch (value.state) {
    case 'sending':
      return typeof value.encoding === 'string' && typeof value.fps === 'number'
        ? {
            state: 'sending',
            encoding: value.encoding as SlimEncoding,
            fps: value.fps,
          }
        : null;
    case 'waiting':
    case 'failed': {
      const failure = decodeFailure(value.failure);
      return failure ? { state: value.state, failure } : null;
    }
    case 'paused':
      return { state: 'paused' };
    default:
      return { state: 'starting' };
  }
}

function decodeSlimWorker(value: unknown): SlimWorker | null {
  if (!isObject(value) || typeof value.phase !== 'string') return null;
  switch (value.phase) {
    case 'running': {
      const link = decodeSlimLink(value.link);
      return typeof value.capturing === 'boolean' && link
        ? { phase: 'running', capturing: value.capturing, link }
        : null;
    }
    case 'stopping':
      return { phase: 'stopping' };
    default:
      return decodeCommonPhase(value);
  }
}

/** The package's own fields, by its `flavor`; an unknown flavor is malformed. */
function decodePackage(state: Fields): Package | null {
  switch (state.flavor) {
    case 'full': {
      const worker = decodeFullWorker(state.worker);
      return typeof state.lighting === 'string' && worker
        ? {
            flavor: 'full',
            lighting: known(targets, state.lighting, 'off'),
            worker,
          }
        : null;
    }
    case 'slim': {
      const worker = decodeSlimWorker(state.worker);
      return typeof state.enabled === 'boolean' && worker
        ? { flavor: 'slim', enabled: state.enabled, worker }
        : null;
    }
    default:
      return null;
  }
}

function isPreferences(value: unknown): value is Preferences {
  return (
    isObject(value) &&
    (value.autostart === undefined || typeof value.autostart === 'boolean') &&
    optionalText(value.instance)
  );
}

/**
 * A state as this app reads it, or `null` when it is malformed. Its
 * structure is checked; a value newer than this app in a known field has
 * one reading: an unknown power or video is `unknown`, an unknown failure
 * kind `other`, an unknown lighting target `off`, an unknown worker phase
 * stopped and an unknown slim link state starting, so an app older than its
 * service still shows the service it talks to.
 */
export function decodeState(value: unknown): State | null {
  if (!isObject(value)) return null;
  const pack = decodePackage(value);
  const failure =
    value.failure === undefined ? undefined : decodeFailure(value.failure);
  const { log, preferences } = value;
  if (
    !pack ||
    failure === null ||
    typeof value.power !== 'string' ||
    typeof value.video !== 'string' ||
    !isPreferences(preferences) ||
    !Array.isArray(log) ||
    !(log as unknown[]).every((line) => typeof line === 'string')
  )
    return null;
  return {
    ...pack,
    power: known(powers, value.power, 'unknown'),
    video: known(videos, value.video, 'unknown'),
    preferences,
    log: log as string[],
    ...(failure ? { failure } : {}),
  };
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
  const state = decodeState(reply);
  if (!state)
    throw new LunaError(
      'invalid_reply',
      'The TV service sent an unexpected state.',
    );
  return state;
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
