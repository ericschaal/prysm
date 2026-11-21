/**
 * An in-memory TV supervisor behind a fake service bridge, so tests and the
 * demo pages run the real Luna client. It follows the supervisor's rules
 * (docs/plans/prysm-v1-webos-supervisor.md §5–6) loosely: no worker, power or
 * timing, but the same replies, validation messages and subscription.
 */
import type {
  BridgeFactory,
  Flavor,
  FullWorker,
  LightingTarget,
  Power,
  Preferences,
  ServiceBridge,
  SlimEncoding,
  SlimWorker,
  State,
  Video,
  WorkerFailure,
} from '@prysm/webos-platform';

/** Where the worker process is, as the supervisor tracks it. */
export type FakeProcess =
  | { phase: 'stopped' }
  | { phase: 'running' }
  | { phase: 'stopping' }
  | { phase: 'retrying'; seconds: number };

/**
 * What the fake supervisor knows, as the real one does: the TV, the worker
 * process and the worker's last report. The published state is built from
 * it per package: a report shows only while the worker runs (the full
 * portal also while it stops).
 */
interface CommonFacts {
  preferences: Preferences;
  power: Power;
  video: Video;
  failure: WorkerFailure | null;
  log: string[];
  worker: FakeProcess;
  capturing: boolean;
  fps: number | null;
}
export interface FullFacts extends CommonFacts {
  flavor: 'full';
  lighting: LightingTarget;
  portal: string | null;
}
export interface SlimFacts extends CommonFacts {
  flavor: 'slim';
  enabled: boolean;
  encoding: SlimEncoding | null;
  paused: boolean;
}
export type FakeFacts = FullFacts | SlimFacts;
/**
 * A change to the facts. Only the package's own keys are accepted (checked
 * when applied, for the untyped tests too): a slim supervisor has no
 * `lighting` or `portal`, a full one no `enabled`, `encoding` or `paused`.
 */
export type FakeChange =
  Partial<Omit<FullFacts, 'flavor'>> | Partial<Omit<SlimFacts, 'flavor'>>;

const commonKeys = [
  'preferences',
  'power',
  'video',
  'failure',
  'log',
  'worker',
  'capturing',
  'fps',
];
const ownKeys: Record<Flavor, string[]> = {
  full: ['lighting', 'portal'],
  slim: ['enabled', 'encoding', 'paused'],
};

/** `change` applied to `facts`; a key of the other package throws. */
function apply(facts: FakeFacts, change: FakeChange) {
  for (const key of Object.keys(change))
    if (!commonKeys.includes(key) && !ownKeys[facts.flavor].includes(key))
      throw new Error(`The ${facts.flavor} supervisor has no \`${key}\`.`);
  Object.assign(facts, change);
}

function initialFacts(flavor: Flavor, preferences?: Preferences): FakeFacts {
  const common: CommonFacts = {
    preferences: { autostart: false, instance: null, ...preferences },
    power: 'active',
    video: 'available',
    failure: null,
    log: [],
    worker: { phase: flavor === 'full' ? 'running' : 'stopped' },
    capturing: false,
    fps: null,
  };
  return flavor === 'full'
    ? {
        ...common,
        flavor,
        lighting: 'off',
        portal: 'http://192.168.1.5:8787/',
      }
    : { ...common, flavor, enabled: false, encoding: null, paused: false };
}

function fullWorker(facts: FullFacts): FullWorker {
  const { worker, capturing, fps, portal } = facts;
  switch (worker.phase) {
    case 'running':
      return { phase: 'running', capturing, fps, portal };
    case 'stopping':
      return { phase: 'stopping', portal };
    default:
      return worker;
  }
}

function slimWorker(facts: SlimFacts): SlimWorker {
  const { worker, capturing, fps, encoding, paused } = facts;
  return worker.phase === 'running'
    ? { phase: 'running', capturing, fps, encoding, paused }
    : worker;
}

/** The `getState` payload the facts make, as the supervisor publishes it. */
function publishedState(facts: FakeFacts): State {
  const { preferences, power, video, failure, log } = facts;
  const common = { preferences, power, video, failure, log };
  return facts.flavor === 'full'
    ? {
        ...common,
        flavor: 'full',
        lighting: facts.lighting,
        worker: fullWorker(facts),
      }
    : {
        ...common,
        flavor: 'slim',
        enabled: facts.enabled,
        worker: slimWorker(facts),
      };
}

/** Lighting is not off (full), or frames should flow (slim). */
const enabled = (facts: FakeFacts) =>
  facts.flavor === 'full' ? facts.lighting !== 'off' : facts.enabled;

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
  /** The next call to this method fails with this text. */
  const failures = new Map<string, string>();
  let setupWorks = true;

  function later(work: () => void) {
    if (options.delayMs) setTimeout(work, options.delayMs);
    else void Promise.resolve().then(work);
  }
  const reply = (value: object) =>
    JSON.stringify({ returnValue: true, ...value });
  const error = (errorText: string) =>
    JSON.stringify({ returnValue: false, errorText });
  const state = () => publishedState(facts);

  /** `setEnabled`, in the package's terms, as the machine does. */
  function setEnabled(value: boolean) {
    if (facts.flavor === 'slim') facts.enabled = value;
    else if (!value) facts.lighting = 'off';
    else if (facts.lighting === 'off') facts.lighting = 'processed';
  }

  /** Settle what follows from the intent, as the machine would. */
  function reconcile() {
    const awake = facts.power === 'active' || facts.power === 'screen_saver';
    const wanted =
      awake &&
      (facts.flavor === 'full' ||
        (facts.enabled && !!facts.preferences.instance));
    facts.worker = { phase: wanted ? 'running' : 'stopped' };
    facts.capturing =
      wanted && enabled(facts) && facts.video === 'available' && !facts.failure;
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
      respond(error(`Unknown service name: "io.prysm.tv.service"`));
      return;
    }
    const failure = failures.get(method);
    if (failure !== undefined) {
      failures.delete(method);
      respond(error(failure));
      return;
    }
    switch (method) {
      case 'getState':
        if (payload.subscribe === true) subscribers.add(bridge);
        respond(reply({ ...state(), subscribed: payload.subscribe === true }));
        return;
      case 'setPreferences': {
        const preferences = payload as Preferences;
        if (facts.flavor === 'full' && preferences.instance) {
          respond(error('instance: only the slim package forwards frames'));
          return;
        }
        // http(s)://host[:port], nothing after it (the TV resolves a name).
        if (
          preferences.instance &&
          !/^https?:\/\/[^/?#\s@]+\/?$/.test(preferences.instance)
        ) {
          respond(
            error(
              `invalid setPreferences request: Prysm address \`${preferences.instance}\`: needs only the address, without a path`,
            ),
          );
          return;
        }
        facts.preferences = {
          autostart: false,
          instance: null,
          ...preferences,
        };
        reconcile();
        respond(reply(state()));
        later(() => publish());
        return;
      }
      case 'setEnabled':
        setEnabled(payload.enabled === true);
        reconcile();
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
        respond(error(`unknown method \`${method}\``));
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
    subscribers: () => subscribers.size,
    failNext(method: string, errorText: string) {
      failures.set(method, errorText);
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
