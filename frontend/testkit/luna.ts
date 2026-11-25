/**
 * An in-memory TV supervisor behind a fake service bridge, so tests and the
 * demo pages run the real Luna client. It follows the supervisor's rules
 * (docs/developers/webos.md: the state machine and the Luna API) loosely:
 * no worker, power or timing, but the same replies, validation messages and
 * subscription.
 */
import {
  unusableSettings,
  type ApplicationSnapshot,
} from '@prysm/application-client';
import {
  APP_ID,
  isEnabled,
  type BridgeFactory,
  type ErrorKind,
  type Flavor,
  type FullWorker,
  type LightingTarget,
  type OutputIssue,
  type Power,
  type Preferences,
  type Problem,
  type ServiceBridge,
  type StreamEncoding,
  type SlimLink,
  type SlimWorker,
  type State,
  type Video,
} from '@prysm/webos-platform';

/** Where the worker process is, as the supervisor tracks it. */
export type FakeProcess =
  | { phase: 'stopped' }
  | { phase: 'running' }
  | { phase: 'stopping' }
  | { phase: 'asking'; problem: Problem };

/** Slim: a link state that says why it waits or does not send. */
export type SaidLink = Extract<SlimLink, { message: string }>;

/**
 * What the fake supervisor knows, as the real one does: the TV, the worker
 * process and the worker's last report. The published state is built from
 * it per package: a report shows only while the worker runs (the full
 * portal also while it stops), and a `null` fact is omitted.
 */
interface CommonFacts {
  preferences: Preferences;
  power: Power;
  video: Video;
  /** Unreadable saved preferences, until preferences are saved. */
  unreadable: string | null;
  /**
   * What a stopping worker's exit will ask, or (full) what the running
   * worker asks; while asking, the process carries it.
   */
  problem: Problem | null;
  log: string[];
  worker: FakeProcess;
  capturing: boolean;
  fps: number | null;
}
export interface FullFacts extends CommonFacts {
  flavor: 'full';
  target: LightingTarget;
  /** Why the lights don't show screen sync, as the running worker reports it. */
  lights: OutputIssue | null;
  portal: string | null;
}
export interface SlimFacts extends CommonFacts {
  flavor: 'slim';
  enabled: boolean;
  encoding: StreamEncoding | null;
  paused: boolean;
  /** Connected, but the TV refuses to share the video it shows. */
  refused: boolean;
  /** The link's wait or failure, when neither paused, refused nor sending. */
  uplink: SaidLink | null;
}
export type FakeFacts = FullFacts | SlimFacts;
/**
 * A change to the facts. Only the package's own keys are accepted (checked
 * when applied, for the untyped tests too): a slim supervisor has no
 * `target`, `lights` or `portal`, a full one no `enabled`, `encoding`, `paused` or
 * `refused`.
 */
export type FakeChange =
  Partial<Omit<FullFacts, 'flavor'>> | Partial<Omit<SlimFacts, 'flavor'>>;

/** A package's own facts: those it does not share with the other. */
type Own<Facts> = Exclude<keyof Facts, keyof CommonFacts | 'flavor'>;
/** Keyed by every fact, so the compiler keeps them in step with the types. */
const commonKeys: Record<keyof CommonFacts, true> = {
  preferences: true,
  power: true,
  video: true,
  unreadable: true,
  problem: true,
  log: true,
  worker: true,
  capturing: true,
  fps: true,
};
const fullKeys: Record<Own<FullFacts>, true> = {
  target: true,
  lights: true,
  portal: true,
};
const slimKeys: Record<Own<SlimFacts>, true> = {
  enabled: true,
  encoding: true,
  paused: true,
  refused: true,
  uplink: true,
};
const ownKeys: Record<Flavor, object> = { full: fullKeys, slim: slimKeys };

/** `change` applied to `facts`; a key of the other package throws. */
function apply(facts: FakeFacts, change: FakeChange) {
  const known = [
    ...Object.keys(commonKeys),
    ...Object.keys(ownKeys[facts.flavor]),
  ];
  for (const key of Object.keys(change))
    if (!known.includes(key))
      throw new Error(`The ${facts.flavor} supervisor has no \`${key}\`.`);
  Object.assign(facts, change);
}

function initialFacts(flavor: Flavor, preferences?: Preferences): FakeFacts {
  const common: CommonFacts = {
    preferences: { autostart: false, ...preferences },
    power: 'active',
    video: 'available',
    unreadable: null,
    problem: null,
    log: [],
    worker: { phase: flavor === 'full' ? 'running' : 'stopped' },
    capturing: false,
    fps: null,
  };
  return flavor === 'full'
    ? {
        ...common,
        flavor,
        target: 'off',
        lights: null,
        portal: 'http://192.168.1.5:8787/',
      }
    : {
        ...common,
        flavor,
        enabled: false,
        encoding: null,
        paused: false,
        refused: false,
        uplink: null,
      };
}

/** `{ [key]: value }`, or nothing for `null` (the service omits it). */
function present<K extends string, V>(key: K, value: V | null) {
  return (value === null ? {} : { [key]: value }) as Partial<Record<K, V>>;
}

function fullWorker(facts: FullFacts): FullWorker {
  const { worker, capturing, fps, lights, portal, problem } = facts;
  switch (worker.phase) {
    case 'running':
      return {
        phase: 'running',
        capturing,
        ...present('fps', fps),
        ...present('lights', lights),
        ...present('portal', portal),
        ...present('problem', problem),
      };
    case 'stopping':
      return {
        phase: 'stopping',
        ...present('portal', portal),
        ...present('problem', problem),
      };
    default:
      return worker;
  }
}

/** The link the facts make: paused, refused, sending in an encoding, its wait or failure, else starting. */
function slimLink(facts: SlimFacts): SlimLink {
  const { fps, encoding, paused, refused, uplink } = facts;
  if (paused) return { state: 'paused' };
  if (refused) return { state: 'refused' };
  if (encoding !== null) return { state: 'sending', encoding, fps: fps ?? 0 };
  return uplink ?? { state: 'starting' };
}

function slimWorker(facts: SlimFacts): SlimWorker {
  const { worker, capturing, problem } = facts;
  switch (worker.phase) {
    case 'running':
      return { phase: 'running', capturing, link: slimLink(facts) };
    case 'stopping':
      return { phase: 'stopping', ...present('problem', problem) };
    default:
      return worker;
  }
}

/** The `getState` payload the facts make, as the supervisor publishes it. */
function publishedState(facts: FakeFacts): State {
  const { preferences, power, video, unreadable, log } = facts;
  const common = {
    preferences,
    power,
    video,
    ...present('unreadable', unreadable),
    log,
  };
  return facts.flavor === 'full'
    ? {
        ...common,
        flavor: 'full',
        target: facts.target,
        worker: fullWorker(facts),
      }
    : {
        ...common,
        flavor: 'slim',
        enabled: facts.enabled,
        worker: slimWorker(facts),
      };
}

export interface FakeSupervisorOptions {
  flavor: Flavor;
  preferences?: Preferences;
  state?: FakeChange;
  /** The service is not registered: every call fails like LS2 does. */
  offline?: boolean;
  /** Reply after this many milliseconds (default: next microtask). */
  delayMs?: number;
}

export interface LunaCall {
  uri: string;
  method: string;
  payload: Record<string, unknown>;
}

export function fakeTvSupervisor(options: FakeSupervisorOptions) {
  let offline = !!options.offline;
  const facts = initialFacts(options.flavor, options.preferences);
  if (options.state) apply(facts, options.state);
  const calls: LunaCall[] = [];
  const subscribers = new Set<FakeBridge>();
  /** Calls held until `release()`, when `hold` is set. */
  const held: (() => void)[] = [];
  let hold = false;
  /** The next call to this method fails with this kind and text. */
  const failures = new Map<
    string,
    { errorKind: ErrorKind; errorText: string }
  >();
  let setupWorks = true;

  function later(work: () => void) {
    if (options.delayMs) setTimeout(work, options.delayMs);
    else void Promise.resolve().then(work);
  }
  const reply = (value: object) =>
    JSON.stringify({ returnValue: true, ...value });
  /** LS2's own error reply (no service, lost subscription): no `errorKind`. */
  const error = (errorText: string) =>
    JSON.stringify({ returnValue: false, errorText });
  /** The service's error reply. */
  const refuse = (errorKind: ErrorKind, errorText: string) =>
    JSON.stringify({ returnValue: false, errorKind, errorText });
  const state = () => publishedState(facts);

  /**
   * Settle what follows from the intent, as the machine would. A choice of
   * the person (`answered`) answers what was asked; nothing else ends
   * asking.
   */
  function reconcile(answered: boolean) {
    if (answered) facts.problem = null;
    else if (facts.worker.phase === 'asking') return;
    const awake = facts.power === 'active' || facts.power === 'screen_saver';
    const wanted =
      awake &&
      (facts.flavor === 'full' ||
        (facts.enabled && !!facts.preferences.prysm_address));
    facts.worker = { phase: wanted ? 'running' : 'stopped' };
    facts.capturing =
      wanted &&
      isEnabled(state()) &&
      facts.video === 'available' &&
      !facts.problem;
    facts.fps = facts.capturing ? 30 : null;
  }

  function publish(change: FakeChange = {}) {
    apply(facts, change);
    for (const bridge of Array.from(subscribers))
      bridge.deliver(reply({ ...state(), subscribed: true }));
  }

  function handle(
    bridge: FakeBridge,
    uri: string,
    payload: Record<string, unknown>,
  ) {
    const method = uri.split('/').pop() ?? '';
    calls.push({ uri, method, payload });
    const respond = (text: string) => later(() => bridge.deliver(text));
    if (uri.startsWith('luna://org.webosbrew.hbchannel.service/')) {
      if (setupWorks) {
        offline = false;
        respond(reply({ stdoutString: 'set up' }));
      } else
        respond(
          JSON.stringify({
            returnValue: false,
            errorText: 'exit 1',
            stderrString: 'setup: cannot write manifests',
          }),
        );
      return;
    }
    if (offline) {
      respond(error(`Unknown service name: "${APP_ID}.service"`));
      return;
    }
    const failure = failures.get(method);
    if (failure !== undefined) {
      failures.delete(method);
      respond(refuse(failure.errorKind, failure.errorText));
      return;
    }
    switch (method) {
      case 'getState':
        if (payload.subscribe === true) subscribers.add(bridge);
        respond(reply({ ...state(), subscribed: payload.subscribe === true }));
        return;
      case 'setPreferences': {
        const preferences = payload as Preferences;
        // http(s)://host[:port], nothing after it (the TV resolves a name).
        if (
          preferences.prysm_address &&
          !/^https?:\/\/[^/?#\s@]+\/?$/.test(preferences.prysm_address)
        ) {
          respond(
            refuse(
              'invalid_request',
              `invalid setPreferences request: Prysm address \`${preferences.prysm_address}\`: needs only the address, without a path`,
            ),
          );
          return;
        }
        if (facts.flavor === 'full' && preferences.prysm_address) {
          respond(
            refuse(
              'invalid_preferences',
              'prysm_address: only the slim package forwards frames',
            ),
          );
          return;
        }
        facts.preferences = { autostart: false, ...preferences };
        facts.unreadable = null;
        reconcile(false);
        respond(reply(state()));
        later(() => publish());
        return;
      }
      case 'setTarget':
        if (facts.flavor !== 'full') {
          respond(
            refuse('wrong_package', 'setTarget is only for the full package'),
          );
          return;
        }
        facts.target = payload.target as LightingTarget;
        reconcile(true);
        respond(reply(state()));
        later(() => publish());
        return;
      case 'setEnabled':
        if (facts.flavor !== 'slim') {
          respond(
            refuse('wrong_package', 'setEnabled is only for the slim package'),
          );
          return;
        }
        facts.enabled = payload.enabled === true;
        reconcile(true);
        respond(reply(state()));
        later(() => publish());
        return;
      case 'uninstall':
        respond(JSON.stringify({ returnValue: true }));
        later(() => {
          offline = true;
          for (const subscriber of Array.from(subscribers))
            subscriber.deliver(error('Service stopped'));
          subscribers.clear();
        });
        return;
      default:
        respond(refuse('unknown_method', `unknown method \`${method}\``));
    }
  }

  class FakeBridge implements ServiceBridge {
    onservicecallback: ((text: string) => void) | null = null;
    cancelled = false;
    deliver(text: string) {
      if (!this.cancelled) this.onservicecallback?.(text);
    }
    call(uri: string, text: string) {
      const payload = JSON.parse(text) as Record<string, unknown>;
      if (hold) held.push(() => handle(this, uri, payload));
      else handle(this, uri, payload);
    }
    cancel() {
      this.cancelled = true;
      subscribers.delete(this);
    }
  }

  const bridge: BridgeFactory = () => new FakeBridge();
  return {
    bridge,
    calls,
    /** The state as `getState` publishes it now. */
    state,
    /** What the supervisor knows now, the state's source. */
    facts: (): Readonly<FakeFacts> => facts,
    /** Change what the supervisor reports, as a worker or power change would. */
    publish,
    /**
     * Full: what the running worker reports of its engine's state, as the
     * real one does (`hosts/webos/src/link.rs`): the session's failure, or
     * else the last surface failure, with what is asked as the engine
     * classifies it (settings it cannot use: change them; anything else:
     * try again). Its target is left to the commands, so the service lags
     * as it does.
     */
    report(snapshot: ApplicationSnapshot<unknown, unknown>) {
      if (facts.flavor !== 'full' || facts.worker.phase !== 'running') return;
      const session =
        'failure' in snapshot.session ? snapshot.session.failure : undefined;
      const surfaces = snapshot.surface_failures;
      const failure = session ?? surfaces[surfaces.length - 1];
      const problem: Problem | null = failure
        ? {
            asked: {
              step: 'ask',
              fix: unusableSettings(failure) ? 'change_settings' : 'try_again',
            },
            message: failure.message,
          }
        : null;
      if (JSON.stringify(problem) !== JSON.stringify(facts.problem))
        publish({ problem });
    },
    subscribers: () => subscribers.size,
    failNext(
      method: string,
      errorText: string,
      errorKind: ErrorKind = 'internal',
    ) {
      failures.set(method, { errorKind, errorText });
    },
    /** End every subscription with an error, as a stopped service does. */
    drop(errorText: string) {
      for (const subscriber of Array.from(subscribers))
        subscriber.deliver(error(errorText));
      subscribers.clear();
    },
    setOffline(value: boolean) {
      offline = value;
    },
    setSetupWorks(value: boolean) {
      setupWorks = value;
    },
    /** Hold every call until `release()`. */
    hold() {
      hold = true;
    },
    release() {
      hold = false;
      for (const run of held.splice(0)) run();
    },
  };
}
