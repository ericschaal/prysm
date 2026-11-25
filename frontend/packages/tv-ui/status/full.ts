/**
 * The full package's status card: what it is about, in order, and its
 * button. The lights are this TV's own portal's.
 */
import {
  pictureOf,
  unreachable,
  unusableSettings,
  type LightingFailure,
  type LightingSituation,
  type OutputIssue,
} from '@prysm/application-client';
import {
  fullLights,
  fullPortal,
  isCapturing,
  isEnabled,
  type LightingTarget,
  type State,
} from '@prysm/webos-platform';
import {
  failed,
  noLights,
  tvSituation,
  type Button,
  type FullState,
  type NoPicture,
  type PortalFacts,
  type SharedSituation,
} from './situation';

/**
 * Full: why the lights don't show screen sync, or that they don't answer
 * whatever was chosen, as the lights card says it: by the portal's state
 * once it is known (while screen sync runs or the lights are waited for),
 * as the worker last said before.
 */
function lightsIssue(
  state: State,
  portal: PortalFacts | null,
): OutputIssue | null {
  if (!portal?.lighting) return fullLights(state);
  const { kind } = portal.lighting;
  return kind === 'sync' || kind === 'unanswered'
    ? (portal.issue ?? null)
    : null;
}

/** The target a running, starting or waiting situation was asked for. */
function targetOf(
  lighting: Exclude<
    LightingSituation,
    { kind: 'faulted' | 'failed' | 'unknown' }
  >,
): LightingTarget {
  switch (lighting.kind) {
    case 'off':
      return 'off';
    case 'sync':
      return 'screen_sync';
    case 'effects':
      return 'effects';
    case 'unanswered':
    case 'starting':
      return lighting.target;
    default:
      return unreachable(lighting);
  }
}

/** Full: what the status card is about ({@link fullSituation}). */
export type FullSituation =
  | SharedSituation
  | NoPicture
  | { kind: 'sessionFailed' }
  | { kind: 'faulted' }
  | {
      kind: 'lightingFailed';
      lighting: Extract<LightingSituation, { kind: 'failed' }>;
    }
  | { kind: 'newer' }
  | { kind: 'off'; failure: LightingFailure | null }
  | { kind: 'effects' }
  /** `target`: what was chosen; with Off or effects, only `unreachable`. */
  | { kind: 'lightsIssue'; issue: OutputIssue; target: LightingTarget }
  | { kind: 'syncOn' }
  | { kind: 'starting' };

/**
 * Full: what the status card is about, by the portal's situation once it is
 * known. The first that applies is said.
 */
export function fullSituation(
  state: FullState,
  portal: PortalFacts | null,
): FullSituation {
  const phase = state.worker.phase;
  const lighting = portal?.lighting;
  if (phase === 'stopping') return { kind: 'stopping' };
  // The worker reports its session's failure, so while the portal's
  // situation is known it says what the failure means (off with a switch
  // off that failed is still off). Before the portal answers, the failure
  // is said only for a mode, so lights that are off never flicker as
  // stopped; Luna does not say whose failure it is, so one of the
  // supervisor's own waits for the portal too. A restart being retried is
  // the supervisor's own.
  if (failed(state)) {
    if (phase === 'retrying') return { kind: 'retrying' };
    if (!lighting && isEnabled(state)) return { kind: 'sessionFailed' };
  }
  if (noLights(portal)) return { kind: 'setUpLights' };
  // A failure the service reports is said first.
  if (phase === 'unknown') return { kind: 'updating' };
  // A faulted or failed session comes before the TV's power and video. It
  // is what needs the user (restart Prysm, try again, another mode); waking
  // the TV or a picture would not end it, and saying "Waiting for video"
  // over it would hide it. Lights that are fine (off, effects, screen sync)
  // let power and video speak first.
  if (lighting?.kind === 'faulted') return { kind: 'faulted' };
  if (lighting?.kind === 'failed') return { kind: 'lightingFailed', lighting };
  const tv = tvSituation(state);
  if (tv) return tv;
  if (lighting?.kind === 'unknown' || (!lighting && state.target === 'unknown'))
    return { kind: 'newer' };
  // The supervisor hears of a change made in the portal a moment later.
  const target = lighting ? targetOf(lighting) : state.target;
  // The lights don't show what was chosen; the lights card says why, as
  // for a refused video. With the lights off or their effects, only lights
  // that don't answer: the target stays and is applied once they answer.
  const issue = lightsIssue(state, portal);
  if (issue === 'unreachable' && (target === 'off' || target === 'effects'))
    return { kind: 'lightsIssue', issue, target };
  if (target === 'off')
    return {
      kind: 'off',
      failure: lighting?.kind === 'off' ? lighting.failure : null,
    };
  if (target === 'effects') return { kind: 'effects' };
  // Screen sync runs (also while it waits for lights that don't answer).
  if (issue) return { kind: 'lightsIssue', issue, target: 'screen_sync' };
  // The portal's measure; it alone says why no picture arrives.
  const picture =
    lighting?.kind === 'sync' && lighting.input
      ? pictureOf(lighting.input)
      : undefined;
  // Without the portal's measure yet, the TV's own word.
  const flowing = lighting
    ? lighting.kind === 'sync' &&
      (picture ? picture === 'arriving' : isCapturing(state))
    : isCapturing(state);
  if (flowing) return { kind: 'syncOn' };
  if (picture === 'refused')
    return { kind: 'waitingForVideo', picture: 'refused' };
  // Lighting is on but no frames arrive: the lights are released to WLED.
  if (fullPortal(state) && phase === 'running')
    return { kind: 'waitingForVideo', picture: 'quiet' };
  return { kind: 'starting' };
}

/**
 * Full: the modes start and stop the lights, so the button only restarts
 * after a failure the supervisor does not retry by itself: again the
 * target that failed, only a mode, and only while the portal's situation,
 * when known, is that failure. Nothing to start or stop in a phase this
 * app cannot read.
 */
export function fullButton(
  state: FullState,
  portal: PortalFacts | null,
): Button {
  const phase = state.worker.phase;
  if (phase === 'unknown' || phase === 'stopping' || phase === 'retrying')
    return 'none';
  if (!failed(state) || !isEnabled(state)) return 'none';
  const lighting = portal?.lighting;
  if (!lighting) return 'retry';
  if (lighting.kind !== 'failed') return 'none';
  // Trying again cannot help settings Prysm can't use.
  return unusableSettings(lighting.failure) ? 'none' : 'retry';
}
