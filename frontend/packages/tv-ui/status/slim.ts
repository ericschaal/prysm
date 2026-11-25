/**
 * The slim package's status card: what it is about, in order, and its
 * button. The lights are those of Prysm on a computer.
 */
import { isCapturing, isPaused, isRefused } from '@prysm/webos-platform';
import type { SavedPreferences } from '../preferences';
import {
  failed,
  noLights,
  tvSituation,
  type Button,
  type NoPicture,
  type PortalFacts,
  type SharedSituation,
  type SlimState,
} from './situation';

/** Slim: what the status card is about ({@link slimSituation}). */
export type SlimSituation =
  | SharedSituation
  | NoPicture
  | { kind: 'encoder' }
  | { kind: 'version' }
  | { kind: 'replaced' }
  | { kind: 'receiverFailed' }
  | { kind: 'captureFailed' }
  | { kind: 'noAddress' }
  | { kind: 'captureOff' }
  | { kind: 'paused' }
  | { kind: 'waitingForPrysm' }
  | { kind: 'sending' };

/**
 * Slim: what the status card is about, by the stream to Prysm on the
 * computer. The first that applies is said.
 */
export function slimSituation(
  state: SlimState,
  portal: PortalFacts | null,
): SlimSituation {
  const phase = state.worker.phase;
  const failure = state.failure?.kind;
  if (phase === 'stopping') return { kind: 'stopping' };
  if (failed(state)) {
    // The encoding is Prysm's setting, so the fix is in its portal.
    if (failure === 'encoder') return { kind: 'encoder' };
    // Prysm and this app speak different stream versions; the TV keeps
    // checking (every second), so updating the older side is enough.
    if (failure === 'version') return { kind: 'version' };
    // Another TV took the stream over; this one stopped until capture is
    // started again, so the two never take it from each other in turn.
    if (failure === 'replaced') return { kind: 'replaced' };
    // Prysm's lighting failed with this TV's stream (no decoder, a broken
    // keyframe); the TV keeps trying, and Prysm's portal says why.
    if (failure === 'receiver_failed') return { kind: 'receiverFailed' };
    if (phase === 'retrying') return { kind: 'retrying' };
    return { kind: 'captureFailed' };
  }
  // Only once this TV knows its Prysm.
  if (noLights(portal) && state.preferences.prysm_address)
    return { kind: 'setUpLights' };
  // A failure the service reports is said first.
  if (phase === 'unknown') return { kind: 'updating' };
  const tv = tvSituation(state);
  if (tv) return tv;
  if (!state.preferences.prysm_address) return { kind: 'noAddress' };
  if (!state.enabled) return { kind: 'captureOff' };
  // Not a failure: lighting is off on the computer, so the TV stops capturing.
  if (isPaused(state)) return { kind: 'paused' };
  // Prysm is not receiving; the worker keeps trying, not a failure.
  if (failure === 'receiver') return { kind: 'waitingForPrysm' };
  if (isRefused(state)) return { kind: 'waitingForVideo', picture: 'refused' };
  if (isCapturing(state)) return { kind: 'sending' };
  return { kind: 'waitingForVideo', picture: 'quiet' };
}

/**
 * Slim: capture on or off, or the address it needs first (`draft`, the
 * preferences as shown). Nothing to start or stop in a phase this app
 * cannot read.
 */
export function slimButton(state: SlimState, draft: SavedPreferences): Button {
  switch (state.worker.phase) {
    case 'unknown':
      return 'none';
    case 'stopping':
      return 'wait';
    default:
      if (state.enabled) return 'stop';
      return draft.prysm_address ? 'start' : 'address';
  }
}
