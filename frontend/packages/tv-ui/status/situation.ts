/**
 * What the status card decides from, in both packages: the portal's facts,
 * the situations they share and the button's actions.
 */
import type {
  LightingSituation,
  OutputIssue,
  Picture,
} from '@prysm/application-client';
import {
  isCapturing,
  isEnabled,
  type Asked,
  type Fix,
  type State,
} from '@prysm/webos-platform';

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
  /**
   * Why the lights don't show screen sync, or that they don't answer Off
   * or their effects, by the portal's own state.
   */
  issue?: OutputIssue | null;
}

export type FullState = Extract<State, { flavor: 'full' }>;
export type SlimState = Extract<State, { flavor: 'slim' }>;

/** The situations of both packages; their words differ only by package. */
export type SharedSituation =
  | { kind: 'stopping' }
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
 * What the button can do about what the service asks: try again (full: the
 * target again; slim: capture stopped and started), set up the service, or
 * nothing that could help.
 */
export type Remedy = Extract<Button, 'retry' | 'setup' | 'none'>;

/** Keyed by every fix, so the compiler keeps them in step. */
const fixRemedies: Record<Fix, Remedy> = {
  try_again: 'retry',
  // The settings change in the portal; trying again cannot help them.
  change_settings: 'none',
  set_up: 'setup',
  unsupported: 'none',
  // A worker that did not end holds the capture until the TV restarts.
  restart_tv: 'none',
};

/** What the button can do about `asked`: lights that stopped answering are tried again. */
export const remedyOf = (asked: Asked): Remedy =>
  asked.step === 'unanswered' ? 'retry' : fixRemedies[asked.fix];

/**
 * The portal has no lights yet: their setup is the one next step, after a
 * stopping or failed worker. Lights are set up in the portal (its first-run
 * setup), never here.
 */
export const noLights = (portal: PortalFacts | null) =>
  !!portal && !portal.lights;

/**
 * What the TV itself says, or `null`: settings it could not read (after
 * what the service asks and every failure of the lights, which need the
 * user first; before power and video, which do not; nothing stopped for
 * it), then, while on, its power and video.
 */
export function tvSituation(state: State): SharedSituation | null {
  if (state.unreadable !== undefined) return { kind: 'unreadable' };
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
