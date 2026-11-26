import type { Failure } from '@prysm/contracts/application';

/**
 * What the state stream does after it failed with each kind: only a lost
 * connection reconnects. Any other kind is Prysm's answer, and asking again
 * gets the same answer.
 */
export const streamStep: Record<Failure['kind'], 'reconnect' | 'stop'> = {
  // Prysm restarts, or the network returns: see `reconnectDelayMs`.
  unavailable: 'reconnect',
  // The portal refuses this page's origin for as long as it runs.
  forbidden: 'stop',
  // A reply this page can't read; a reload brings a page that can.
  internal: 'stop',
  // The portal holds as many streams as it allows; nothing says when one ends.
  busy: 'stop',
  // This Prysm has no such stream; a reload brings a page that matches it.
  not_found: 'stop',
  method_not_allowed: 'stop',
  // Answers to a request body or an upgrade; the stream sends neither.
  bad_request: 'stop',
  request_timeout: 'stop',
  payload_too_large: 'stop',
  unsupported_media_type: 'stop',
  upgrade_required: 'stop',
  // Answers to a command; the stream says the session's state instead.
  invalid: 'stop',
  stale_revision: 'stop',
  adapter: 'stop',
  needs_screen_sync: 'stop',
  faulted: 'stop',
};

/**
 * How long a stream of the portal (state, colours) waits before it
 * reconnects after `attempt` failures in a row since it last received
 * (0 for the first): 250 ms doubling to 5 s. Nothing announces Prysm's
 * return, and it listens on the same address once it restarts, so trying
 * again is how the page finds it: soon after a restart, then every 5 s
 * through an outage.
 */
export const reconnectDelayMs = (attempt: number) =>
  Math.min(5000, 250 * 2 ** attempt);

/** How long after a lost connection the stream reconnected `tries` times. */
function reconnectedBy(tries: number) {
  let total = 0;
  for (let attempt = 0; attempt < tries; attempt++)
    total += reconnectDelayMs(attempt);
  return total;
}

/**
 * How long a view keeps the last state while the stream is away: until it
 * reconnected four times, and the fourth had as long as the first wait to
 * answer (4 s). A restart is back by then; a longer wait hides an outage.
 */
export const STALE_MS = reconnectedBy(4) + reconnectDelayMs(0);
