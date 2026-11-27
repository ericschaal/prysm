/**
 * The Prysm that drives the lights, over its HTTP API: the full package's own
 * portal on this TV, or the slim package's remote Prysm
 * (`preferences.prysm_address`).
 */
import { useEffect, useMemo, useState } from 'react';
import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/application';
import {
  failureOf,
  STALE_MS,
  terminalStreamFailure,
  type ApplicationClient,
  type ApplicationSnapshot,
  type LightingTarget,
} from '@prysm/application-client';
import { useConnection } from '@prysm/application-react';
import {
  createApplicationRuntime,
  type ApplicationRuntime,
} from '@prysm/application-state';

/**
 * The portal's state with the sections any Prysm sends (`{ kind, ... }`):
 * the full package's own, or the slim package's computer's.
 */
export type PortalSnapshot = ApplicationSnapshot<
  CaptureSettings,
  OutputSettings
>;
type Runtime = ApplicationRuntime<CaptureSettings, OutputSettings>;

export interface PortalOptions {
  /** An application client for the portal at `baseUrl` (an origin). */
  client: (
    baseUrl: string,
  ) => ApplicationClient<CaptureSettings, OutputSettings>;
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
  return (
    typeof window.AbortController === 'function' &&
    typeof window.TextDecoder === 'function' &&
    typeof window.ReadableStream === 'function' &&
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
  const connection = useConnection(runtime);
  useEffect(
    () => (runtime && active ? runtime.connect() : undefined),
    [runtime, active],
  );

  const error = connection?.status === 'connected' ? null : connection?.error;
  const stopped = terminalStreamFailure(error);
  const refused = stopped?.kind === 'forbidden';
  const message = error && !stopped ? error.message : null;
  /** Whether anything answers at `baseUrl`, for the lost `message`. */
  const [probed, setProbed] = useState<{
    message: string;
    reachable: boolean;
  } | null>(null);
  useEffect(() => {
    if (!message || !baseUrl || !probe) return;
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
  }, [message, baseUrl, probe]);
  let reach: Reach = 'unknown';
  if (refused) reach = 'refused';
  else if (message && probed?.message === message)
    reach = probed.reachable ? 'refused' : 'unreachable';

  const live = connection?.status === 'connected' ? connection.snapshot : null;
  // The last state of this runtime, until STALE_MS after its stream left
  // (lost, or released while the page was hidden), counted while shown.
  const [kept, setKept] = useState<{
    runtime: Runtime;
    snapshot: PortalSnapshot;
    expired: boolean;
  } | null>(null);
  if (live && runtime && kept?.snapshot !== live)
    setKept({ runtime, snapshot: live, expired: false });
  const stale =
    !live && kept?.runtime === runtime && !kept.expired ? kept : null;
  useEffect(() => {
    if (!stale || !active) return;
    const timer = setTimeout(
      () => setKept((last) => last && { ...last, expired: true }),
      STALE_MS,
    );
    return () => clearTimeout(timer);
  }, [stale, active]);
  const snapshot: PortalSnapshot | null = live ?? stale?.snapshot ?? null;
  return {
    baseUrl,
    supported,
    runtime,
    connection,
    snapshot,
    reach,
    /** Why the stream stopped, said by Prysm; it does not reconnect. */
    stopped: stopped?.message ?? null,
    /** Rejects when there is no portal to command (unsupported, no address). */
    setTarget: (target: LightingTarget) =>
      runtime
        ? runtime.commands.setTarget(target)
        : Promise.reject(new Error('Prysm isn’t connected.')),
    /** End calibration: the lights return to the saved processing. */
    endCalibration: () =>
      runtime ? runtime.commands.setCalibration(null) : Promise.resolve(),
  };
}
export type PortalView = ReturnType<typeof usePortal>;
