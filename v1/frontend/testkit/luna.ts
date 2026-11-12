/**
 * An in-memory TV supervisor behind a fake service bridge, so tests and the
 * demo pages run the real Luna client. It follows the supervisor's rules
 * (docs/plans/prysm-v1-webos-supervisor.md §5–6) loosely: no worker, power or
 * timing, but the same replies, validation messages and subscription.
 */
import type {
  BridgeFactory,
  Flavor,
  Preferences,
  ServiceBridge,
  State,
} from '@prysm/webos-platform';

export interface FakeSupervisorOptions {
  flavor: Flavor;
  preferences?: Preferences;
  state?: Partial<State>;
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

const slimDefaults = { width: 320, height: 180, fps: 30, include_ui: true };

export function fakeTvSupervisor(options: FakeSupervisorOptions) {
  const full = options.flavor === 'full';
  let offline = !!options.offline;
  let state: State = {
    flavor: options.flavor,
    preferences: {
      autostart: false,
      instance: null,
      receiver: null,
      capture: { ...slimDefaults },
      ...options.preferences,
    },
    enabled: false,
    lighting: full ? 'off' : null,
    power: 'active',
    video: 'available',
    worker: full ? 'running' : 'stopped',
    capturing: false,
    portal: full ? 'http://192.168.1.5:8787/' : null,
    failure: null,
    log: [],
    ...options.state,
  };
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
  const reply = (value: object) => JSON.stringify({ returnValue: true, ...value });
  const error = (errorText: string) => JSON.stringify({ returnValue: false, errorText });

  /** Settle what follows from the intent, as the machine would. */
  function reconcile() {
    const wanted =
      state.power === 'active' || state.power === 'screen_saver'
        ? full || (state.enabled && !!state.preferences.receiver)
        : false;
    state.worker = wanted ? 'running' : 'stopped';
    if (full) state.lighting = state.enabled ? (state.lighting === 'off' ? 'processed' : state.lighting) : 'off';
    state.capturing = wanted && state.enabled && state.video === 'available' && !state.failure;
  }

  function publish(change: Partial<State> = {}) {
    state = { ...state, ...change };
    for (const bridge of Array.from(subscribers))
      bridge.deliver(reply({ ...state, subscribed: true }));
  }

  function handle(bridge: FakeBridge, uri: string, payload: Record<string, unknown>) {
    const method = uri.split('/').pop() ?? '';
    calls.push({ uri, method, payload });
    const respond = (text: string) => later(() => bridge.deliver(text));
    if (uri.startsWith('luna://org.webosbrew.hbchannel.service/')) {
      if (setupWorks) {
        offline = false;
        respond(reply({ stdoutString: 'set up' }));
      } else respond(JSON.stringify({ returnValue: false, errorText: 'exit 1', stderrString: 'setup: cannot write manifests' }));
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
        respond(reply({ ...state, subscribed: payload.subscribe === true }));
        return;
      case 'setPreferences': {
        const preferences = payload as Preferences;
        if (full && (preferences.instance || preferences.receiver)) {
          respond(error('instance, receiver: only the slim package forwards frames'));
          return;
        }
        if (preferences.instance && !/^https?:\/\/[^/?#\s]+$/.test(preferences.instance)) {
          respond(error('instance: must be an origin such as http://192.168.1.20:8787'));
          return;
        }
        if (preferences.receiver && !/^(\d+\.\d+\.\d+\.\d+|\[[0-9a-f:]+\]):\d+$/i.test(preferences.receiver)) {
          respond(error(`invalid setPreferences request: invalid socket address syntax`));
          return;
        }
        state = {
          ...state,
          preferences: { autostart: false, instance: null, receiver: null, capture: { ...slimDefaults }, ...preferences },
        };
        reconcile();
        respond(reply(state));
        later(() => publish());
        return;
      }
      case 'setEnabled':
        state = { ...state, enabled: payload.enabled === true };
        reconcile();
        respond(reply(state));
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
    state: () => state,
    /** Change what the supervisor reports, as a worker or power change would. */
    publish,
    subscribers: () => subscribers.size,
    failNext(method: string, errorText: string) {
      failures.set(method, errorText);
    },
    /** End every subscription with an error, as a stopped service does. */
    drop(errorText: string) {
      for (const subscriber of Array.from(subscribers)) subscriber.deliver(error(errorText));
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
