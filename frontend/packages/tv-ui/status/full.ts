/**
 * The full package's status card: what it is about, in order, and its
 * button. The lights are this TV's own portal's.
 */
import {
  pictureOf,
  unreachable,
  type LightingFailure,
  type LightingMode,
  type LightingSituation,
  type OutputIssue,
} from '@prysm/application-client';
import {
  fullLights,
  fullPortal,
  isCapturing,
  problemOf,
  type Asked,
  type LightingTarget,
  type State,
} from '@prysm/webos-platform';
import {
  noLights,
  remedyOf,
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
 * once it is known (while screen sync runs), as the worker last said
 * before.
 */
function lightsIssue(
  state: State,
  portal: PortalFacts | null,
): OutputIssue | null {
  if (!portal?.lighting) return fullLights(state);
  return portal.lighting.kind === 'sync' ? (portal.issue ?? null) : null;
}

/**
 * Full: what the lights do or what stopped, the headline of a failure: the
 * portal's word while it answers, else the target's (with no worker,
 * Prysm itself stopped).
 */
export type Headline =
  | 'prysmStopped'
  | 'syncStopped'
  | 'syncNotStarted'
  | 'effectsStopped'
  | 'off'
  | 'effectsOn'
  | 'syncOn';

function headlineOf(
  state: FullState,
  lighting: LightingSituation | undefined,
): Headline {
  const said = lighting && lightingHeadline(lighting);
  if (said) return said;
  switch (state.target) {
    case 'screen_sync':
      return 'syncStopped';
    case 'effects':
      return 'effectsStopped';
    case 'off':
      return state.worker.phase === 'asking' ? 'prysmStopped' : 'off';
    case 'unknown':
      return 'prysmStopped';
    default:
      return unreachable(state.target);
  }
}

/** What the lights do or what failed, by the portal; `null` when it says neither. */
function lightingHeadline(lighting: LightingSituation): Headline | null {
  switch (lighting.kind) {
    case 'failed':
      if (lighting.target === 'effects') return 'effectsStopped';
      return lighting.fromEffects ? 'syncNotStarted' : 'syncStopped';
    case 'off':
      return 'off';
    case 'effects':
      return 'effectsOn';
    case 'sync':
      return 'syncOn';
    case 'faulted':
    case 'unknown':
    case 'unanswered':
    case 'starting':
      return null;
    default:
      return unreachable(lighting);
  }
}

/** Full: what the status card is about ({@link fullSituation}). */
export type FullSituation =
  | SharedSituation
  | NoPicture
  /** What the service asks, under what the lights do or what stopped. */
  | { kind: 'problem'; asked: Asked; headline: Headline }
  | { kind: 'faulted' }
  /** The portal says it failed; the service does not say yet what is asked. */
  | { kind: 'lightingFailed'; headline: Headline }
  | { kind: 'newer' }
  | { kind: 'off'; failure: LightingFailure | null }
  | { kind: 'effects' }
  /** `target`: what was chosen; with Off or effects, only `unreachable`. */
  | { kind: 'lightsIssue'; issue: OutputIssue; target: LightingTarget }
  | { kind: 'syncOn' }
  | { kind: 'starting'; target: LightingMode };

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
  const problem = problemOf(state);
  // A stop that asks says what it asks; any other is a restart.
  if (phase === 'stopping' && !problem) return { kind: 'stopping' };
  if (noLights(portal)) return { kind: 'setUpLights' };
  if (phase === 'unknown') return { kind: 'updating' };
  // What the service asks, or a faulted or failed session, comes before
  // the TV's power and video. It is what needs the user; waking the TV or
  // a picture would not end it, and saying "Waiting for video" over it
  // would hide it. Lights that are fine (off, effects, screen sync) let
  // power and video speak first. The service's word is the one fix, also
  // with the lights off, and before lights the portal says it waits for:
  // Prysm stopped trying.
  if (problem)
    return {
      kind: 'problem',
      asked: problem.asked,
      headline: headlineOf(state, lighting),
    };
  if (lighting?.kind === 'faulted') return { kind: 'faulted' };
  if (lighting?.kind === 'failed')
    return { kind: 'lightingFailed', headline: headlineOf(state, lighting) };
  const tv = tvSituation(state);
  if (tv) return tv;
  if (lighting?.kind === 'unknown' || (!lighting && state.target === 'unknown'))
    return { kind: 'newer' };
  // A mode asked for is starting until the session reaches it.
  if (lighting?.kind === 'starting')
    return { kind: 'starting', target: lighting.target };
  // With the lights off or their effects, lights that don't answer: the
  // target stays and is applied once they answer.
  if (lighting?.kind === 'unanswered')
    return {
      kind: 'lightsIssue',
      issue: 'unreachable',
      target: lighting.target,
    };
  // Before the portal's first snapshot, the TV service's target; it hears
  // of a change made in the portal a moment later.
  const target = lighting ? null : state.target;
  // The lights don't show what was chosen; the lights card says why, as
  // for a refused video.
  const issue = lightsIssue(state, portal);
  if (issue === 'unreachable' && (target === 'off' || target === 'effects'))
    return { kind: 'lightsIssue', issue, target };
  if (lighting?.kind === 'off' || target === 'off')
    return {
      kind: 'off',
      failure: lighting?.kind === 'off' ? lighting.failure : null,
    };
  if (lighting?.kind === 'effects' || target === 'effects')
    return { kind: 'effects' };
  // Screen sync runs (also while it waits for lights that don't answer).
  if (issue) return { kind: 'lightsIssue', issue, target: 'screen_sync' };
  // The portal's measure; it alone says why no picture arrives.
  const picture = lighting?.input ? pictureOf(lighting.input) : undefined;
  // Without the portal's measure yet, the TV's own word.
  const flowing = picture ? picture === 'arriving' : isCapturing(state);
  if (flowing) return { kind: 'syncOn' };
  if (picture === 'refused')
    return { kind: 'waitingForVideo', picture: 'refused' };
  // Lighting is on but no frames arrive: the lights are released to WLED.
  if (fullPortal(state) && phase === 'running')
    return { kind: 'waitingForVideo', picture: 'quiet' };
  return { kind: 'starting', target: 'screen_sync' };
}

/**
 * Full: the modes start and stop the lights, so the button only answers
 * what the card says the service asks, as its fix says: try again (the
 * target again), set up the service, or nothing that could help. Nothing
 * while the worker stops, or for a target newer than this app (nothing to
 * send).
 */
export function fullButton(state: FullState, situation: FullSituation): Button {
  if (situation.kind !== 'problem' || state.worker.phase === 'stopping')
    return 'none';
  const remedy = remedyOf(situation.asked);
  return remedy === 'retry' && state.target === 'unknown' ? 'none' : remedy;
}
