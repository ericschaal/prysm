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
  failureOf,
  type ApplicationClient,
  type ApplicationSnapshot,
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

/** How long {@link checkPortal} waits for the portal's state. */
export const CHECK_TIMEOUT_MS = 10_000;

/**
 * Whether a Prysm portal answers at `baseUrl`, once (the slim package checks
 * an address when it is saved): `ok`, `refused` (it answers but does not
 * allow this app's origin) or `unreachable`. A browser without what the HTTP
 * client needs can only tell `unreachable` from `ok`, through the probe.
 */
export async function checkPortal(
  baseUrl: string,
  options: PortalOptions,
): Promise<'ok' | 'refused' | 'unreachable'> {
  const reachable = async () =>
    options.probe ? options.probe(baseUrl).catch(() => false) : false;
  if (!(options.supported ?? portalSupported()))
    return !options.probe || (await reachable()) ? 'ok' : 'unreachable';
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), CHECK_TIMEOUT_MS);
  try {
    const observation = await options
      .client(baseUrl)
      .observe(controller.signal);
    observation.close();
    return 'ok';
  } catch (reason) {
    if (failureOf(reason).kind === 'forbidden') return 'refused';
    // A CORS refusal looks like a network error; the probe ignores CORS.
    return (await reachable()) ? 'refused' : 'unreachable';
  } finally {
    clearTimeout(timer);
  }
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
    () =>
      baseUrl && supported ? createApplicationRuntime(client(baseUrl)) : null,
    [baseUrl, supported, client],
  );
  const subscribe = useCallback(
    (changed: () => void) =>
      runtime ? runtime.store.subscribe(changed) : () => {},
    [runtime],
  );
  const connection: Connection<unknown, unknown> | null = useSyncExternalStore(
    subscribe,
    () =>
      runtime
        ? runtime.selectors.selectConnection(runtime.store.getState())
        : null,
  );
  useEffect(
    () => (runtime && active ? runtime.connect() : undefined),
    [runtime, active],
  );

  // Kept while the runtime retries, until a state arrives.
  const error = connection?.status === 'connected' ? null : connection?.error;
  const refused = error?.kind === 'forbidden';
  const message = error?.message ?? null;
  /** Whether anything answers at `baseUrl`, for the error `message`. */
  const [probed, setProbed] = useState<{
    message: string;
    reachable: boolean;
  } | null>(null);
  useEffect(() => {
    if (!message || refused || !baseUrl || !probe) return;
    let current = true;
    probe(baseUrl).then(
      (reachable) => {
        if (current) setProbed({ message, reachable });
      },
      () => {
        if (current) setProbed({ message, reachable: false });
      },
    );
    return () => {
      current = false;
    };
  }, [message, refused, baseUrl, probe]);
  let reach: Reach = 'unknown';
  if (refused) reach = 'refused';
  else if (message && probed?.message === message)
    reach = probed.reachable ? 'refused' : 'unreachable';

  const snapshot: Snapshot | null =
    connection?.status === 'connected' ? connection.snapshot : null;
  return {
    baseUrl,
    supported,
    runtime,
    connection,
    snapshot,
    reach,
    /** Rejects when there is no portal to command (unsupported, no address). */
    setTarget: (target: LightingTarget) =>
      runtime
        ? runtime.store.dispatch(runtime.commands.setTarget(target))
        : Promise.reject(new Error('Prysm is not connected.')),
    /** End calibration: the lights return to the saved processing. */
    endCalibration: () =>
      runtime
        ? runtime.store.dispatch(runtime.commands.setCalibration(null))
        : Promise.resolve(),
  };
}
export type PortalView = ReturnType<typeof usePortal>;
