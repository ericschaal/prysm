/**
 * What the status card decides from, in both packages: the portal's facts,
 * the situations they share and the button's actions.
 */
import type {
  LightingSituation,
  OutputIssue,
  Picture,
} from '@prysm/application-client';
import { isCapturing, isEnabled, type State } from '@prysm/webos-platform';

/**
 * The primary button is about the service on this TV only; what the lights
 * do is chosen with the lighting modes. Full package: set up, or try again
 * after a failure, else no button. Slim: capture on or off.
 */
export type PrimaryAction =
  'none' | 'setup' | 'retry' | 'disable' | 'address' | 'enable';

/** What the TV UI knows from the portal that drives the lights. */
export interface PortalFacts {
  /** The portal's settings have an output. */
  lights: boolean;
  /** Where people open the portal, if known. */
  address: string | null;
  /**
   * What the lights do by the portal's own state: the full package's
   * headline follows it, so it never disagrees with the lights card.
   */
  lighting?: LightingSituation;
  /** Why the lights don't show screen sync, by the portal's own state. */
  issue?: OutputIssue | null;
}

export type FullState = Extract<State, { flavor: 'full' }>;
export type SlimState = Extract<State, { flavor: 'slim' }>;

/** The situations of both packages; their words differ only by package. */
export type SharedSituation =
  | { kind: 'stopping' }
  | { kind: 'retrying' }
  | { kind: 'setUpLights' }
  | { kind: 'updating' }
  | { kind: 'unreadable' }
  | { kind: 'asleep'; power: Exclude<State['power'], 'active'> }
  | { kind: 'noVideo' };

/** The capture gives no picture, and why (not a failure). */
export type NoPicture = {
  kind: 'waitingForVideo';
  picture: Exclude<Picture, 'arriving'>;
};

/** The primary button: what it does, as `buttons` labels it. */
export type Button =
  'none' | 'wait' | 'setup' | 'retry' | 'stop' | 'address' | 'start';

/**
 * The service reports a failure that stopped something. Slim: Prysm not
 * receiving (`receiver`) is a wait, the worker keeps trying, not a failure.
 * The saved preferences could not be read (`preferences`): the service runs
 * with the defaults, and saving a setting replaces the file. Nothing
 * stopped.
 */
export function failed(state: State): boolean {
  const kind = state.failure?.kind;
  return !!kind && kind !== 'receiver' && kind !== 'preferences';
}

/**
 * The portal has no lights yet: their setup is the one next step, after a
 * stopping or failed worker. Lights are set up in the portal (its first-run
 * setup), never here.
 */
export const noLights = (portal: PortalFacts | null) =>
  !!portal && !portal.lights;

/**
 * What the TV itself says, or `null`: settings it could not read (after
 * every failure of the lights or the worker, which need the user first;
 * before power and video, which do not), then, while on, its power and
 * video.
 */
export function tvSituation(state: State): SharedSituation | null {
  if (state.failure?.kind === 'preferences') return { kind: 'unreadable' };
  if (!isEnabled(state)) return null;
  if (state.power !== 'active') return { kind: 'asleep', power: state.power };
  if (state.video === 'unavailable') return { kind: 'noVideo' };
  return null;
}

/** Frames flow from an awake TV with video. */
export const ready = (state: State) =>
  isCapturing(state) &&
  state.power === 'active' &&
  state.video !== 'unavailable';
