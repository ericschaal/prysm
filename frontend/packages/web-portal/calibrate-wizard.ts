/**
 * The Calibrate page's state: which screen shows, the TV request in flight
 * and the problem to show. Pure, so the transitions are tested on their own.
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
  { section: 'white', pattern: 'gray-192' },
  { section: 'red', pattern: 'red-pair' },
  { section: 'green', pattern: 'green-pair' },
  { section: 'blue', pattern: 'blue-pair' },
  { section: 'checks', pattern: 'yellow', fixes: ['red', 'green'] },
  { section: 'checks', pattern: 'cyan', fixes: ['green', 'blue'] },
  { section: 'checks', pattern: 'magenta', fixes: ['red', 'blue'] },
  { section: 'checks', pattern: 'coast', fixes: everything },
  { section: 'checks', pattern: 'garden', fixes: everything },
  { section: 'checks', pattern: 'dusk', fixes: everything },
  { section: 'review', pattern: 'review-grid' },
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

export interface WizardState {
  phase: WizardPhase;
  /** A TV request is on its way. */
  busy: boolean;
  problem: string;
  /** The TV showed a pattern since the last request, so its absence means it ended. */
  seen: boolean;
}

export type WizardAction =
  | { type: 'sourceChosen'; from: string }
  /** Start or Resume: wait for the TV to show a pattern again. */
  | { type: 'patternRequested' }
  | { type: 'patternSeen' }
  | { type: 'requestSent' }
  /**
   * The TV shows the pattern of `screen` (the current one when absent),
   * opened from the check `returnTo` if given.
   */
  | { type: 'patternShown'; screen?: number; returnTo?: number }
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
    case 'requestSent':
      return { ...state, busy: true, problem: '' };
    case 'patternShown':
      return {
        ...state,
        busy: false,
        phase:
          phase.kind === 'calibrating' && action.screen !== undefined
            ? {
                ...phase,
                screen: action.screen,
                returnTo: action.returnTo ?? null,
              }
            : phase,
      };
    case 'requestFailed':
      return { ...state, busy: false, problem: action.problem };
    case 'started':
      return {
        ...state,
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
