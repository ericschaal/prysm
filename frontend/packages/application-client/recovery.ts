import type { Failure } from '@prysm/contracts/application';

/** Recovery after the application state stream fails. */
export const streamStep: Record<Failure['kind'], 'reconnect' | 'stop'> = {
  unavailable: 'reconnect',
  forbidden: 'stop',
  internal: 'stop',
  busy: 'stop',
  not_found: 'stop',
  method_not_allowed: 'stop',
  bad_request: 'stop',
  request_timeout: 'stop',
  payload_too_large: 'stop',
  unsupported_media_type: 'stop',
  upgrade_required: 'stop',
  invalid: 'stop',
  stale_revision: 'stop',
  adapter: 'stop',
  needs_screen_sync: 'stop',
  faulted: 'stop',
};

/** The failure that stopped observation, or null while the stream can reconnect. */
export function terminalStreamFailure(error: Failure | null | undefined) {
  return error && streamStep[error.kind] !== 'reconnect' ? error : null;
}

/** Retry delay in milliseconds, starting at attempt zero after the last event. */
export const reconnectDelayMs = (attempt: number) =>
  Math.min(5000, 250 * 2 ** attempt);

/** How long after a lost connection the stream reconnected `tries` times. */
function reconnectedBy(tries: number) {
  let total = 0;
  for (let attempt = 0; attempt < tries; attempt++)
    total += reconnectDelayMs(attempt);
  return total;
}

/** Keep the last state through four retries and one initial response interval. */
export const STALE_MS = reconnectedBy(4) + reconnectDelayMs(0);
