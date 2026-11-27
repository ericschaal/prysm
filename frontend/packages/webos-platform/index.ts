import {
  errorKinds,
  fixes,
  powers,
  slimLinkStates,
  videos,
} from '@prysm/contracts/constants';
/**
 * The TV supervisor's Luna API (`luna://<APP_ID>.service`, both packages)
 * over the webOS service bridge. Values are the generated `luna` contract;
 * replies are checked before use because the bridge hands over plain text.
 * See docs/developers/webos.md, "Luna API".
 */
import {
  type Fields,
  isObject,
  known,
  lightingTargets,
} from '@prysm/application-client';
import type * as Luna from '@prysm/contracts/luna';
import type * as Wire from '@prysm/contracts/luna-reading';
import {
  isLunaStateReply,
  isLunaErrorReply,
  isLunaDoneReply,
} from '@prysm/contracts/validate/luna';

export type {
  Asked,
  ErrorKind,
  Fix,
  FullWorker,
  LightingTarget,
  OutputIssue,
  Power,
  Preferences,
  Problem,
  StreamEncoding,
  SlimWorker,
  Video,
} from '@prysm/contracts/luna';

export type TargetReading = Luna.LightingTarget | 'unknown';
type UnknownPhase = { phase: 'unknown' };
type FullReading =
  | Exclude<Luna.FullWorker, { phase: 'running' }>
  | (Omit<Extract<Wire.FullWorker, { phase: 'running' }>, 'problem'> & {
      problem?: Luna.Problem;
    })
  | UnknownPhase;
type SlimReading =
  | Exclude<Luna.SlimWorker, { phase: 'running' }>
  | (Omit<Extract<Wire.SlimWorker, { phase: 'running' }>, 'link'> & {
      link: SlimLink;
    })
  | UnknownPhase;

export type State = ReturnType<typeof readState>;
export type Flavor = State['flavor'];
export type Worker = State['worker'];
export type SlimLink = Extract<
  Wire.SlimLink,
  { state: (typeof slimLinkStates)[number] }
>;

/**
 * A known mode is on (full: screen sync or effects; a target newer than
 * this app is not counted as on), or frames should flow (slim).
 */
export function isEnabled(state: State): boolean {
  return state.flavor === 'full'
    ? state.target === 'screen_sync' || state.target === 'effects'
    : state.enabled;
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

/**
 * Full: why the running worker's lights are not showing screen sync, or
 * that they don't answer whatever was chosen; else `null`. An issue newer
 * than this app is kept as sent.
 */
export function fullLights(state: State): string | null {
  if (state.flavor !== 'full') return null;
  const worker = state.worker;
  return worker.phase === 'running' ? (worker.lights ?? null) : null;
}

/**
 * What the person is asked, in whichever phase says it: while no worker
 * runs, while one stops that will not be restarted, or (full) what the
 * running worker asks; else `null`.
 */
export function problemOf(state: State): Luna.Problem | null {
  const { worker } = state;
  if (worker.phase === 'asking') return worker.problem;
  if (worker.phase === 'stopping') return worker.problem ?? null;
  return state.flavor === 'full' && state.worker.phase === 'running'
    ? (state.worker.problem ?? null)
    : null;
}

/** Slim: the running worker's stream to Prysm; else `null`. */
export function slimLink(state: State): SlimLink | null {
  if (state.flavor !== 'slim') return null;
  const worker = state.worker;
  return worker.phase === 'running' ? worker.link : null;
}

/** Slim: the encoding the running worker sends in; else `null`. */
export function slimEncoding(state: State): string | null {
  const link = slimLink(state);
  return link?.state === 'sending' ? link.encoding : null;
}

/** Slim: the receiver paused the running worker's stream (its lights are off). */
export function isPaused(state: State): boolean {
  return slimLink(state)?.state === 'paused';
}

/** Slim: connected, but the TV refuses to share the video it shows. */
export function isRefused(state: State): boolean {
  return slimLink(state)?.state === 'refused';
}

/** The TV app of both packages; its service is `<APP_ID>.service`. */
export const APP_ID = 'io.github.ericschaal.prysm';
export const SERVICE_URI = `luna://${APP_ID}.service`;
/** Installed by the IPK; run as root through the Homebrew Channel. */
export const SETUP_SCRIPT = `/media/developer/apps/usr/palm/services/${APP_ID}.service/setup.sh`;
export const HOMEBREW_EXEC = 'luna://org.webosbrew.hbchannel.service/exec';
/** A call or the first subscription reply that takes longer has failed. */
export const CALL_TIMEOUT_MS = 30_000;
/**
 * Delay before a lost subscription is opened again. The service is dynamic
 * (`Type=dynamic`): after setup, a fresh call starts it again (observed
 * after killing it), so a subscription it ended opens on the next try.
 * Without setup the bus cannot start it and answers so to every call
 * (`absent`), and the app says it needs setting up.
 */
export const RESUBSCRIBE_MS = 3_000;

/**
 * The bus's own replies (`errorCode` -1, no `errorKind`) when it cannot give
 * a caller the service, as observed on a webOS TV: `Service does not exist:
 * <name>.` for a name never registered, and `<name> is not running.` with
 * `serviceName` for one installed without setup, also sent to a subscriber
 * when the service ends.
 */
const NOT_REGISTERED_REPLY = 'Service does not exist';
const NOT_RUNNING_REPLY = ' is not running.';

/**
 * - `unavailable`: no bridge (not on a TV), or the bridge failed.
 * - `absent`: the bus cannot give the service (not registered or running).
 * - `timeout`: no reply within {@link CALL_TIMEOUT_MS}.
 * - `refused`: the service answered `returnValue: false` (its `errorText`).
 * - `invalid_reply`: the reply is not what the contract says.
 */
export type LunaFailureKind =
  'unavailable' | 'absent' | 'timeout' | 'refused' | 'invalid_reply';

/**
 * Why the service refused a call (its `errorKind`), as this app reads it:
 * `unknown` is a kind newer than this app, or a reply without one (LS2's
 * own, or not a refusal at all). The message is the one to show.
 */
export type ErrorKindReading = Luna.ErrorKind | 'unknown';

export class LunaError extends Error {
  constructor(
    readonly kind: LunaFailureKind,
    message: string,
    readonly errorKind: ErrorKindReading = 'unknown',
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

declare global {
  interface Window {
    PalmServiceBridge?: new () => ServiceBridge;
    WebOSServiceBridge?: new () => ServiceBridge;
  }
}

/** The TV's bridge, or null when the page is outside a webOS app. */
export function webosBridge(): BridgeFactory | null {
  const Bridge = window.PalmServiceBridge ?? window.WebOSServiceBridge;
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
  setPreferences(preferences: Luna.Preferences): Promise<State>;
  /** Full: what the lights do; the slim package refuses it. */
  setTarget(target: Luna.SetTargetRequest['target']): Promise<State>;
  /** Slim: whether frames flow; the full package refuses it. */
  setEnabled(enabled: boolean): Promise<State>;
  /** Stop the worker, remove the setup and end the service; settings stay. */
  uninstall(): Promise<void>;
  /** Run `setup.sh` as root through the Homebrew Channel (set up or repair). */
  setup(): Promise<void>;
  /** Cancel every call still waiting for a reply. */
  cancelAll(): void;
}

function readProblem({ asked, message }: Wire.Problem): Luna.Problem {
  if (asked.step === 'unanswered')
    return { asked: { step: 'unanswered' }, message };
  const fix =
    asked.step === 'ask' && 'fix' in asked
      ? known(fixes, asked.fix, 'try_again')
      : 'try_again';
  return { asked: { step: 'ask', fix }, message };
}

function optionalProblem(worker: Wire.FullWorker | Wire.SlimWorker) {
  return 'problem' in worker && worker.problem !== undefined
    ? { problem: readProblem(worker.problem) }
    : {};
}

function readCommonWorker(
  worker: Wire.FullWorker | Wire.SlimWorker,
): Extract<FullReading, { phase: 'stopped' | 'asking' | 'unknown' }> {
  if (worker.phase === 'stopped') return { phase: 'stopped' };
  if (
    worker.phase === 'asking' &&
    'problem' in worker &&
    worker.problem !== undefined
  )
    return { phase: 'asking', problem: readProblem(worker.problem) };
  return { phase: 'unknown' };
}

function readFullWorker(worker: Wire.FullWorker): FullReading {
  if (worker.phase === 'running' && 'capturing' in worker) {
    const { problem, ...running } = worker;
    return problem === undefined
      ? running
      : { ...running, problem: readProblem(problem) };
  }
  if (worker.phase === 'stopping') {
    const portal = 'portal' in worker ? worker.portal : undefined;
    return {
      phase: 'stopping',
      ...(portal === undefined ? {} : { portal }),
      ...optionalProblem(worker),
    };
  }
  return readCommonWorker(worker);
}

// The generated validator has already checked every known state's fields.
function isKnownLink(link: Wire.SlimLink): link is SlimLink {
  return slimLinkStates.some((state) => state === link.state);
}

function readSlimLink(link: Wire.SlimLink): SlimLink {
  if (!isKnownLink(link)) return { state: 'starting' };
  switch (link.state) {
    case 'sending':
      return link;
    case 'starting':
    case 'refused':
    case 'paused':
      return { state: link.state };
    default:
      return { state: link.state, message: link.message };
  }
}

function readSlimWorker(worker: Wire.SlimWorker): SlimReading {
  if (worker.phase === 'running' && 'link' in worker)
    return {
      phase: worker.phase,
      capturing: worker.capturing,
      link: readSlimLink(worker.link),
    };
  if (worker.phase === 'stopping')
    return { phase: worker.phase, ...optionalProblem(worker) };
  return readCommonWorker(worker);
}

/** Map open service values to the states this app can display. */
export function readState(value: Wire.State) {
  const { power, video, preferences, log, unreadable } = value;
  const common = {
    power: known(powers, power, 'unknown'),
    video: known(videos, video, 'unknown'),
    preferences,
    log,
    ...(unreadable === undefined ? {} : { unreadable }),
  };
  return value.flavor === 'full'
    ? {
        ...common,
        flavor: value.flavor,
        target: known(lightingTargets, value.target, 'unknown'),
        worker: readFullWorker(value.worker),
      }
    : {
        ...common,
        flavor: value.flavor,
        enabled: value.enabled,
        worker: readSlimWorker(value.worker),
      };
}

type Reply = Fields;

/** The bus cannot give the service; Prysm's own replies carry `errorKind`. */
const absent = ({ errorKind, errorText, serviceName }: Reply) =>
  errorKind === undefined &&
  typeof errorText === 'string' &&
  (errorText.startsWith(NOT_REGISTERED_REPLY) ||
    (typeof serviceName === 'string' &&
      errorText === `${serviceName}${NOT_RUNNING_REPLY}`));

function parseReply(text: string, service: string): Reply {
  const message = `${service} sent an unreadable reply.`;
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw new LunaError('invalid_reply', message);
  }
  if (!isObject(value)) throw new LunaError('invalid_reply', message);
  return value;
}

function decode(text: string): Reply {
  const reply = parseReply(text, 'The TV service');
  if (isLunaDoneReply(reply)) return reply;
  if (!isLunaErrorReply(reply))
    throw new LunaError(
      'invalid_reply',
      'The TV service sent an unexpected reply.',
    );
  const kind =
    reply.errorKind === undefined
      ? 'unknown'
      : known(errorKinds, reply.errorKind, 'unknown');
  throw new LunaError(
    absent(reply) ? 'absent' : 'refused',
    reply.errorText,
    kind,
  );
}

function stateOf(reply: Reply): State {
  if (!isLunaStateReply(reply))
    throw new LunaError(
      'invalid_reply',
      'The TV service sent an unexpected state.',
    );
  return readState(reply);
}

const notOnTv = () =>
  new LunaError(
    'unavailable',
    'Open Prysm on your LG TV to reach its service.',
  );

/** Setup talks to Homebrew Channel, which can answer with the command's stderr. */
function setupReply(text: string) {
  const reply = parseReply(text, 'Homebrew Channel');
  if (reply.returnValue === true) return;
  if (absent(reply))
    throw new LunaError(
      'absent',
      'Setup needs the Homebrew Channel with root access.',
    );
  const detail = [reply.errorText, reply.stderrString].find(
    (value): value is string => typeof value === 'string' && value.length > 0,
  );
  throw new LunaError('refused', detail ?? 'Setup failed.');
}

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
    function cancel() {
      if (!active) return;
      // Cancelling may call back synchronously; ignore it once released.
      active = false;
      clearTimeout(timer);
      pending.delete(cancel);
      channel.cancel();
    }
    const fail = (error: LunaError) => {
      if (!active) return;
      cancel();
      onFailure(error);
    };
    if (timeoutMs !== null)
      timer = setTimeout(
        () => fail(new LunaError('timeout', 'The TV service didn’t respond.')),
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
    parse: (text: string) => T,
  ) {
    return new Promise<T>((resolve, reject) => {
      open(
        uri,
        payload,
        (text) => {
          resolve(parse(text));
          return true;
        },
        reject,
        CALL_TIMEOUT_MS,
      );
    });
  }
  const method = (name: string) => `${SERVICE_URI}/${name}`;

  const stateRequest = (name: string, payload: object) =>
    request(method(name), payload, (text) => stateOf(decode(text)));
  return {
    watch(listener) {
      let stopped = false;
      let close = () => {};
      let retry: ReturnType<typeof setTimeout> | undefined;
      function failed(error: LunaError) {
        if (stopped) return;
        retry = setTimeout(subscribe, RESUBSCRIBE_MS);
        listener({ online: false, error });
      }
      function subscribe() {
        let first = true;
        close = open(
          method('getState'),
          { subscribe: true },
          (text) => {
            const reply = decode(text);
            const state = stateOf(reply);
            if (first && reply.subscribed !== true)
              throw new LunaError(
                'refused',
                'The TV service didn’t accept a subscription.',
              );
            first = false;
            if (!stopped) listener({ online: true, state });
            return false;
          },
          failed,
          CALL_TIMEOUT_MS,
        );
        // The bridge may deliver its first reply before call() returns.
        if (stopped) close();
      }
      subscribe();
      return () => {
        stopped = true;
        clearTimeout(retry);
        close();
      };
    },
    setPreferences: (preferences) =>
      stateRequest('setPreferences', preferences),
    setTarget: (target) => stateRequest('setTarget', { target }),
    setEnabled: (enabled) => stateRequest('setEnabled', { enabled }),
    uninstall: () =>
      request(method('uninstall'), {}, (text) => {
        decode(text);
      }),
    setup: () => request(HOMEBREW_EXEC, { command: SETUP_SCRIPT }, setupReply),
    cancelAll() {
      for (const cancel of Array.from(pending)) cancel();
    },
  };
}
