/**
 * The Calibrate page's state: which screen shows, the TV request in flight
 * and the problem to show. Pure, so the transitions are tested on their own.
 * A tap moves the screen at once and the TV's pattern follows; if the TV
 * refuses, the wizard goes back to what it shows and says why.
 */
import type { Pattern } from '@prysm/application-client';
import type { Primary } from './calibration-model';

export type Section = 'white' | Primary | 'checks' | 'review';
/** What a screen adjusts: white, or one primary. */
export type Adjustable = 'white' | Primary;

export interface Screen {
  section: Section;
  pattern: Pattern;
  /** A check's links back: the screens adjusting what it is made of. */
  fixes?: readonly Adjustable[];
}

const everything = ['white', 'red', 'green', 'blue'] as const;

/**
 * The wizard, one TV pattern per screen: white, then each primary on its
 * pair (the vivid half shows a turn, the moderate half how strong it is,
 * which a full colour cannot show), then checks with no controls of their
 * own: the secondaries name the two primaries they mix, the mixed scenes
 * anything.
 */
export const screens: readonly Screen[] = [
  { section: 'white', pattern: 'gray_192' },
  { section: 'red', pattern: 'red_pair' },
  { section: 'green', pattern: 'green_pair' },
  { section: 'blue', pattern: 'blue_pair' },
  { section: 'checks', pattern: 'yellow', fixes: ['red', 'green'] },
  { section: 'checks', pattern: 'cyan', fixes: ['green', 'blue'] },
  { section: 'checks', pattern: 'magenta', fixes: ['red', 'blue'] },
  { section: 'checks', pattern: 'coast', fixes: everything },
  { section: 'checks', pattern: 'garden', fixes: everything },
  { section: 'checks', pattern: 'dusk', fixes: everything },
  { section: 'review', pattern: 'review_grid' },
];

/** The screen adjusting `what`. */
export const screenOf = (what: Adjustable): number =>
  screens.findIndex((screen) => screen.section === what);

export type WizardPhase =
  /** Before the start: the calibration to copy (`''` for a new one). */
  | { kind: 'start'; from: string }
  | {
      kind: 'calibrating';
      /** The draft calibration. */
      draftId: string;
      /** The active calibration before the start, for Cancel. */
      previous: string | undefined;
      /** Index into {@link screens}. */
      screen: number;
      /** The check that sent here to adjust, to come back to; else `null`. */
      returnTo: number | null;
      /** The name typed on the review screen; `null` keeps the draft's. */
      name: string | null;
    }
  /** Finished or cancelled. */
  | { kind: 'done'; title: string; message: string };

/** Where the wizard is: a screen, the check it came from, and the comparison. */
export interface Place {
  screen: number;
  returnTo: number | null;
  uncorrected: boolean;
}

export interface WizardState {
  phase: WizardPhase;
  /** A TV request is on its way. */
  busy: boolean;
  problem: string;
  /** The TV showed a pattern since the last request, so its absence means it ended. */
  seen: boolean;
  /** The TV shows the pattern without the draft's correction (as asked: shown at once). */
  uncorrected: boolean;
  /**
   * The last place the TV confirmed while the wizard is ahead of it, to go
   * back to if the TV refuses; `null` when nothing is on its way.
   */
  confirmed: Place | null;
}

export type WizardAction =
  | { type: 'sourceChosen'; from: string }
  /** Start or Resume: wait for the TV to show a pattern again. */
  | { type: 'patternRequested' }
  | { type: 'patternSeen' }
  /**
   * A pattern is asked of the TV. The wizard moves at once to `screen` (the
   * current one when absent), opened from the check `returnTo` if given, and
   * shows the comparison `uncorrected`; the TV follows.
   */
  | {
      type: 'requestSent';
      screen?: number;
      returnTo?: number;
      uncorrected?: boolean;
    }
  /**
   * The TV shows a pattern asked for. `more`: a newer request is still on its
   * way, and the TV now shows `place`.
   */
  | { type: 'patternShown'; more?: Place }
  /** The TV refused the newest request: back to what it shows, saying why. */
  | { type: 'requestFailed'; problem: string }
  | { type: 'started'; draftId: string; previous: string | undefined }
  | { type: 'named'; name: string }
  /** Finish or Cancel: calibration is being ended on the TV. */
  | { type: 'ending' }
  | { type: 'ended'; title: string; message: string };

export const initialWizard: WizardState = {
  phase: { kind: 'start', from: '' },
  busy: false,
  problem: '',
  seen: false,
  uncorrected: false,
  confirmed: null,
};

export function wizardReducer(
  state: WizardState,
  action: WizardAction,
): WizardState {
  const { phase } = state;
  switch (action.type) {
    case 'sourceChosen':
      return phase.kind === 'start'
        ? { ...state, phase: { ...phase, from: action.from } }
        : state;
    case 'patternRequested':
      return { ...state, seen: false };
    case 'patternSeen':
      return { ...state, seen: true };
    case 'requestSent': {
      const uncorrected = action.uncorrected ?? false;
      if (phase.kind !== 'calibrating')
        return { ...state, busy: true, problem: '', uncorrected };
      const moved = action.screen !== undefined;
      return {
        ...state,
        busy: true,
        problem: '',
        uncorrected,
        confirmed: state.confirmed ?? {
          screen: phase.screen,
          returnTo: phase.returnTo,
          uncorrected: state.uncorrected,
        },
        phase: moved
          ? {
              ...phase,
              screen: action.screen!,
              returnTo: action.returnTo ?? null,
            }
          : phase,
      };
    }
    case 'patternShown':
      return action.more
        ? { ...state, confirmed: action.more }
        : { ...state, busy: false, confirmed: null };
    case 'requestFailed': {
      const back = state.confirmed;
      return {
        ...state,
        busy: false,
        problem: action.problem,
        confirmed: null,
        ...(back && phase.kind === 'calibrating'
          ? {
              uncorrected: back.uncorrected,
              phase: { ...phase, screen: back.screen, returnTo: back.returnTo },
            }
          : {}),
      };
    }
    case 'started':
      return {
        ...state,
        confirmed: null,
        phase: {
          kind: 'calibrating',
          draftId: action.draftId,
          previous: action.previous,
          screen: 0,
          returnTo: null,
          name: null,
        },
      };
    case 'named':
      return phase.kind === 'calibrating'
        ? { ...state, problem: '', phase: { ...phase, name: action.name } }
        : state;
    case 'ending':
      return { ...state, busy: true };
    case 'ended':
      return {
        ...state,
        busy: false,
        phase: { kind: 'done', title: action.title, message: action.message },
      };
  }
}
