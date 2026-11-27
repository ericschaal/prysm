/**
 * What the status card decides from, in both packages: the portal's facts,
 * the situations they share and the button's actions.
 */
import type { LightingSituation, OutputIssue } from '@prysm/application-client';
import {
  isCapturing,
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
  control?: import('@prysm/contracts/application').LightingControl;
  /** The portal's settings have an output. */
  lights: boolean;
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
  change_settings: 'none',
  set_up: 'setup',
  unsupported: 'none',
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

/** Frames flow from an awake TV with video. */
export const ready = (state: State) =>
  isCapturing(state) &&
  state.power === 'active' &&
  state.video !== 'unavailable';
