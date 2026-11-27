import {
  isCapturing,
  isPaused,
  isRefused,
  problemOf,
  slimLink,
} from '@prysm/webos-platform';
import type { SavedPreferences } from '../preferences';
import {
  noLights,
  remedyOf,
  type Button,
  type PortalFacts,
  type SlimState,
} from './situation';
import {
  note,
  warning,
  noPictureCopy,
  problemCopy,
  stopping,
  setupLights,
  tvCopy,
  updating,
  type Copy,
} from './copy';

export function slimStatus(state: SlimState, portal: PortalFacts | null): Copy {
  const phase = state.worker.phase;
  const problem = problemOf(state);
  if (phase === 'stopping' && !problem) return stopping('slim');
  if (problem) return problemCopy(problem.asked, 'Capture stopped', 'slim');
  // The running worker's own failures, as its link says them.
  const link = slimLink(state)?.state;
  if (link === 'encoder_failed')
    return warning(
      'Encoding not supported',
      'Choose another encoding in Prysm’s web portal.',
    );
  if (link === 'version_differs')
    return warning(
      'Update Prysm or TV app',
      'Update the older of Prysm and this app.',
    );
  if (link === 'receiver_failed')
    return warning(
      'Prysm stopped receiving',
      'Its web portal on your computer says why.',
    );
  if (noLights(portal) && state.preferences.prysm_address) return setupLights;
  if (phase === 'unknown') return updating;
  const tv = tvCopy(state);
  if (tv) return tv;
  if (!state.preferences.prysm_address)
    return note('Set up this TV', 'Add the address of Prysm on your computer.');
  if (!state.enabled)
    return note('Capture off', 'Start capture to send the picture to Prysm.');
  if (isPaused(state))
    return note(
      'Paused by Prysm',
      'It continues when Prysm turns the lights on.',
    );
  if (link === 'waiting')
    return note(
      'Waiting for Prysm',
      'Check that Prysm is running on your computer.',
    );
  if (link === 'capture_waiting')
    return note(
      'Waiting for video',
      'Capture resumes when the video signal changes.',
    );
  if (isRefused(state)) return noPictureCopy('refused', 'slim');
  if (isCapturing(state))
    return note(
      'Sending the picture',
      'Prysm on your computer lights up with it.',
    );
  return noPictureCopy('quiet', 'slim');
}

/**
 * Slim: capture on or off (stopped and started, it also tries again), the
 * setup the service asks for, or the address it needs first (`draft`, the
 * preferences as shown). Nothing to start or stop in a phase this app
 * cannot read.
 */
export function slimButton(state: SlimState, draft: SavedPreferences): Button {
  const problem = problemOf(state);
  switch (state.worker.phase) {
    case 'unknown':
      return 'none';
    case 'stopping':
      return 'wait';
    default: {
      // What the service asks is answered first: its setup, or the same
      // again (another TV took the stream over, the stream failed).
      const remedy = problem && remedyOf(problem.asked);
      if (remedy && remedy !== 'none') return remedy;
      if (state.enabled) return 'stop';
      return draft.prysm_address ? 'start' : 'address';
    }
  }
}
