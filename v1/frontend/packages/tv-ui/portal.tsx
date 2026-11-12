/**
 * The Prysm that drives the lights, over its HTTP API: the full package's own
 * portal on this TV, or the slim package's remote Prysm (`preferences.instance`).
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from 'react';
import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type Failure,
  type LightingTarget,
} from '@prysm/application-client';
import {
  createApplicationRuntime,
  type ApplicationRuntime,
  type Connection,
} from '@prysm/application-state';

type Snapshot = ApplicationSnapshot<unknown, unknown>;
type Runtime = ApplicationRuntime<unknown, unknown>;

export interface PortalOptions {
  /** An application client for the portal at `baseUrl` (an origin). */
  client: (baseUrl: string) => ApplicationClient<unknown, unknown>;
  /**
   * Whether the portal answers at all, ignoring CORS (a `no-cors` request):
   * tells "not allowed from the TV" from "not reachable".
   */
  probe?: (baseUrl: string) => Promise<boolean>;
  /** The browser has what the HTTP client needs; default: detected. */
  supported?: boolean;
}

/** What the HTTP client and the state runtime use beyond Chromium 53. */
export function portalSupported(): boolean {
  const scope = window as unknown as Record<string, unknown>;
  return (
    typeof scope.AbortController === 'function' &&
    typeof scope.TextDecoder === 'function' &&
    typeof scope.ReadableStream === 'function' &&
    typeof Promise.prototype.finally === 'function'
  );
}

export type Reach = 'unknown' | 'refused' | 'unreachable';

export function failureOf(error: unknown): Failure {
  return error instanceof ApplicationError
    ? error.failure
    : {
        kind: 'internal',
        message: error instanceof Error ? error.message : String(error),
      };
}

/** The state stream of the portal at `baseUrl` while `active`. */
export function usePortal(
  baseUrl: string | null,
  options: PortalOptions,
  active: boolean,
) {
  const supported = options.supported ?? portalSupported();
  const { client, probe } = options;
  const runtime = useMemo<Runtime | null>(
    () => (baseUrl && supported ? createApplicationRuntime(client(baseUrl)) : null),
    [baseUrl, supported, client],
  );
  const subscribe = useCallback(
    (changed: () => void) => (runtime ? runtime.store.subscribe(changed) : () => {}),
    [runtime],
  );
  const connection = useSyncExternalStore(subscribe, () =>
    runtime ? runtime.selectors.selectConnection(runtime.store.getState()) : null,
  ) as Connection<unknown, unknown> | null;
  useEffect(() => (runtime && active ? runtime.connect() : undefined), [runtime, active]);

  const [reach, setReach] = useState<{ error: string; reach: Reach } | null>(null);
  // Kept while the runtime retries, until a state arrives.
  const error = connection?.status === 'connected' ? null : (connection?.error ?? null);
  useEffect(() => {
    if (!error || !baseUrl) return;
    let current = true;
    if (/refuses requests from this page|origin denied/i.test(error)) {
      setReach({ error, reach: 'refused' });
      return;
    }
    if (!probe) return;
    probe(baseUrl).then(
      (reachable) => {
        if (current) setReach({ error, reach: reachable ? 'refused' : 'unreachable' });
      },
      () => {
        if (current) setReach({ error, reach: 'unreachable' });
      },
    );
    return () => {
      current = false;
    };
  }, [error, baseUrl, probe]);

  const snapshot: Snapshot | null =
    connection?.status === 'connected' ? connection.snapshot : null;
  return {
    baseUrl,
    supported,
    runtime,
    connection,
    snapshot,
    reach: error && reach?.error === error ? reach.reach : ('unknown' as Reach),
    setTarget: (target: LightingTarget) =>
      runtime!.store.dispatch(runtime!.commands.setTarget(target)),
  };
}
export type PortalView = ReturnType<typeof usePortal>;

const outputKind = (snapshot: Snapshot) =>
  (snapshot.settings.output as { kind?: string } | undefined)?.kind;
const effectsOf = (snapshot: Snapshot) =>
  outputKind(snapshot) === 'wled' ? 'WLED effects' : null;

export function describeLighting(snapshot: Snapshot): string {
  const effects = effectsOf(snapshot) ?? 'Light effects';
  const { session, target, failure } = snapshot;
  switch (session.state) {
    case 'faulted':
      return 'Prysm stopped responding. Restart Prysm to recover.';
    case 'running':
      return 'Screen sync is on.';
    case 'native':
      return `${effects} are on.`;
    case 'stopped':
      if (target === 'off') return 'Lights are off.';
      if (failure) return target === 'processed' ? 'Screen sync stopped.' : `${effects} could not start.`;
      return target === 'processed' ? 'Starting screen sync…' : `Starting ${effects}…`;
  }
}

const captureNames: Record<string, string> = {
  remote: 'a TV (Remote capture)',
  device: 'a video device',
  file: 'a video file',
  webos: 'this TV',
};

/** The lighting card: what the lights do and the three modes. */
export function LightingCard({
  flavor,
  portal,
  address,
}: {
  flavor: 'full' | 'slim';
  portal: PortalView;
  /** The portal's address for people on other devices (slim: as typed). */
  address: string | null;
}) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const [error, setError] = useState('');
  const { snapshot, connection } = portal;
  const where = flavor === 'full' ? 'the Prysm on this TV' : `Prysm at ${address ?? ''}`;

  async function choose(target: LightingTarget) {
    setPending(target);
    setError('');
    try {
      await portal.setTarget(target);
    } catch (reason) {
      setError(failureOf(reason).message);
    } finally {
      setPending(null);
    }
  }

  let status: string;
  let detail: string | null = null;
  let attention = false;
  if (!portal.baseUrl) {
    status =
      flavor === 'full'
        ? 'Waiting for the web portal on this TV…'
        : 'Add the Prysm address to control its lights from here.';
  } else if (!portal.supported) {
    status = 'This TV’s browser is too old to control Prysm from the app.';
    detail = `Open ${address ?? portal.baseUrl} on your phone or computer instead.`;
    attention = true;
  } else if (snapshot) {
    status = describeLighting(snapshot);
    if (snapshot.input && snapshot.session.state === 'running')
      detail = snapshot.input.receiving
        ? `Picture arriving at ${snapshot.input.fps.toFixed(1)} frames per second.`
        : 'No picture arriving yet.';
    if (flavor === 'slim') {
      const kind = (snapshot.settings.capture as { kind?: string } | undefined)?.kind;
      if (kind !== 'remote') {
        detail = `This Prysm takes its picture from ${kind ? (captureNames[kind] ?? kind) : 'nothing yet'}. Choose Remote capture in its portal to use this TV.`;
        attention = true;
      }
    }
  } else if (portal.reach === 'refused') {
    status = `${capitalize(where)} does not allow control from the TV.`;
    detail =
      flavor === 'slim'
        ? "Start Prysm on your computer with --allow-origin null to use these buttons. Capture works without it."
        : 'Restart the TV app; if this persists, check the Prysm service.';
    attention = true;
  } else if (connection?.error) {
    status = `Cannot reach ${where}.`;
    detail =
      portal.reach === 'unreachable'
        ? 'Check that Prysm is running and that the address is right.'
        : connection.error;
    attention = true;
  } else status = `Connecting to ${where}…`;

  const offline = !snapshot || pending !== null;
  const faulted = snapshot?.session.state === 'faulted';
  const effects = snapshot ? effectsOf(snapshot) : null;
  const hasOutput = !!snapshot?.settings.output;
  const hasCapture = !!snapshot?.settings.capture;
  const modes: { value: LightingTarget; label: string; disabled: boolean }[] = [
    { value: 'processed', label: 'Screen sync', disabled: !hasOutput || !hasCapture },
    { value: 'device_native', label: effects ?? 'Light effects', disabled: !effects },
    { value: 'off', label: 'Off', disabled: false },
  ];
  return (
    <section
      className={`card lighting${attention ? ' attention' : ''}`}
      aria-labelledby="lighting-title"
    >
      <h2 id="lighting-title" className="card-title">
        Lights
      </h2>
      <p id="lighting-status" className="lighting-status" role="status" aria-live="polite">
        {status}
      </p>
      {detail ? (
        <p id="lighting-detail" className="lighting-detail">
          {detail}
        </p>
      ) : null}
      <div className="modes" data-row="lighting" role="group" aria-label="Lighting mode" hidden={!snapshot}>
        {modes.map((mode) => (
          <button
            key={mode.value}
            type="button"
            id={`mode-${mode.value}`}
            className="mode"
            aria-pressed={snapshot?.target === mode.value}
            disabled={offline || faulted || mode.disabled}
            onClick={() => void choose(mode.value)}
          >
            {mode.label}
          </button>
        ))}
      </div>
      {snapshot && !hasOutput ? (
        <p className="lighting-detail">Set up your lights in the web portal first.</p>
      ) : null}
      {snapshot?.failure ? (
        <p className="notice error" role="alert">
          {snapshot.failure.message}
        </p>
      ) : null}
      {error && error !== snapshot?.failure?.message ? (
        <p className="notice error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}

const capitalize = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);
