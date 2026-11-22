/**
 * The Calibrate page's state: which screen shows, the TV request in flight,
 * the question Cancel asks, what is being closed and the problem to show.
 * Pure, so every transition is tested on its own. A tap moves the screen at
 * once and the TV's pattern follows; if the TV refuses, the wizard goes back
 * to what it shows and says why.
 */
import { same, type Pattern } from '@prysm/application-client';
import type {
  Calibration,
  CorrectionControls,
} from '@prysm/contracts/application';
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

/** Where the wizard is: a screen, the check it came from, and the comparison. */
export interface Place {
  /** Index into {@link screens}. */
  screen: number;
  /** The check that sent here to adjust, to come back to; else `null`. */
  returnTo: number | null;
  /** The TV shows the pattern without the draft's correction. */
  uncorrected: boolean;
}

/** A tap's way from a place: to `screen` (opened from `returnTo`), or the comparison. */
export interface Move {
  screen?: number;
  returnTo?: number;
  uncorrected?: boolean;
}

/**
 * The place after `move`: a new screen forgets the check it came from unless
 * it names one; the comparison ends with every tap that does not ask for it.
 */
export function placeAfter(place: Place, move: Move): Place {
  const moved = move.screen !== undefined;
  return {
    screen: move.screen ?? place.screen,
    returnTo: moved ? (move.returnTo ?? null) : place.returnTo,
    uncorrected: move.uncorrected ?? false,
  };
}

/** The pattern request on its way to the TV, if any. */
export type Request =
  | { kind: 'idle' }
  | {
      kind: 'sending';
      /** The last place the TV confirmed, to go back to if it refuses. */
      confirmed: Place;
      /** It resumes calibration that ended: refused, it is still ended. */
      resuming: boolean;
    };

/** Finish, Cancel or Leave, under way. */
export type Closing = 'finish' | 'discard' | 'leave';

export interface Calibrating {
  kind: 'calibrating';
  /** The draft calibration. */
  draftId: string;
  /** The active calibration before the start, for Cancel. */
  previous: string | undefined;
  /** The draft's correction at the start: unchanged, Cancel needs no question. */
  startedWith: CorrectionControls;
  /** Shown at once; the TV follows. */
  place: Place;
  /** The name typed on the review screen; `null` keeps the draft's. */
  name: string | null;
  /** The TV showed a pattern since the start or the resume, so its absence means it ended. */
  seen: boolean;
  request: Request;
  /** Cancel asks before discarding changes. */
  confirming: boolean;
  closing: Closing | null;
}

export type WizardPhase =
  /** Before the start: the calibration to copy (`''` for a new one). */
  | { kind: 'start'; from: string }
  /** Start was tapped: the first pattern, then the draft calibration. */
  | { kind: 'starting'; from: string }
  | Calibrating
  /** Calibration is being ended on the TV; `last` is still shown. */
  | { kind: 'ending'; last: Calibrating }
  /** Finished or cancelled. */
  | { kind: 'done'; title: string; message: string };

export interface WizardState {
  phase: WizardPhase;
  /** Why the TV did not show what was asked; `''` when it did. */
  problem: string;
}

export type WizardAction =
  | { type: 'sourceChosen'; from: string }
  | { type: 'startRequested' }
  /** The first pattern or the draft was refused; `problem` says why, if it is the TV's. */
  | { type: 'startFailed'; problem: string }
  | {
      type: 'started';
      draftId: string;
      previous: string | undefined;
      startedWith: CorrectionControls;
    }
  | { type: 'patternSeen' }
  /**
   * A pattern is asked of the TV. The wizard moves at once to `place`; the
   * TV follows. `resume`: calibration ended, and is asked to show again.
   */
  | { type: 'requestSent'; place: Place; resume: boolean }
  /**
   * The TV shows a pattern asked for. `more`: a newer request is still on its
   * way, and the TV now shows `more`.
   */
  | { type: 'patternShown'; more?: Place }
  /** The TV refused the newest request: back to what it shows, saying why. */
  | { type: 'requestFailed'; problem: string }
  | { type: 'named'; name: string }
  /** Cancel with changes: ask first. */
  | { type: 'confirmAsked' }
  /** Keep, the answer to the question. */
  | { type: 'kept' }
  | { type: 'closingStarted'; how: Closing }
  /** The save before closing failed (and says why): the buttons work again. */
  | { type: 'closingFailed' }
  | { type: 'ending' }
  | { type: 'ended'; title: string; message: string };

export const initialWizard: WizardState = {
  phase: { kind: 'start', from: '' },
  problem: '',
};

const firstPlace: Place = { screen: 0, returnTo: null, uncorrected: false };

/**
 * Something would be lost by Cancel: a slider moved or a name typed since
 * the start. Without the draft (deleted elsewhere) nothing is.
 */
export function losesWork(
  phase: Calibrating,
  draft: Calibration | undefined,
): boolean {
  if (!draft) return false;
  return (
    !same(draft.correction, phase.startedWith) ||
    (phase.name !== null && phase.name.trim() !== draft.name)
  );
}

/** Each action, in the phases it belongs to; in any other it changes nothing. */
export function wizardReducer(
  state: WizardState,
  action: WizardAction,
): WizardState {
  const { phase } = state;
  const calibrating = phase.kind === 'calibrating' ? phase : null;
  const change = (next: Partial<Calibrating>): WizardState =>
    calibrating ? { ...state, phase: { ...calibrating, ...next } } : state;
  switch (action.type) {
    case 'sourceChosen':
      return phase.kind === 'start'
        ? { ...state, phase: { kind: 'start', from: action.from } }
        : state;
    case 'startRequested':
      return phase.kind === 'start'
        ? { phase: { kind: 'starting', from: phase.from }, problem: '' }
        : state;
    case 'startFailed':
      return phase.kind === 'starting'
        ? {
            phase: { kind: 'start', from: phase.from },
            problem: action.problem,
          }
        : state;
    case 'started':
      return phase.kind === 'starting'
        ? {
            ...state,
            phase: {
              kind: 'calibrating',
              draftId: action.draftId,
              previous: action.previous,
              startedWith: action.startedWith,
              place: firstPlace,
              name: null,
              seen: false,
              request: { kind: 'idle' },
              confirming: false,
              closing: null,
            },
          }
        : state;
    case 'patternSeen':
      return calibrating && !calibrating.seen ? change({ seen: true }) : state;
    case 'requestSent': {
      if (!calibrating) return state;
      const { request } = calibrating;
      const sending = request.kind === 'sending' ? request : null;
      return {
        ...change({
          place: action.place,
          seen: action.resume ? false : calibrating.seen,
          request: {
            kind: 'sending',
            confirmed: sending?.confirmed ?? calibrating.place,
            resuming: (sending?.resuming ?? false) || action.resume,
          },
        }),
        problem: '',
      };
    }
    case 'patternShown': {
      if (calibrating?.request.kind !== 'sending') return state;
      const request = calibrating.request;
      return change({
        request: action.more
          ? { ...request, confirmed: action.more }
          : { kind: 'idle' },
      });
    }
    case 'requestFailed': {
      if (calibrating?.request.kind !== 'sending') return state;
      const request = calibrating.request;
      return {
        ...change({
          place: request.confirmed,
          request: { kind: 'idle' },
          // A refused resume leaves calibration ended, as it was.
          seen: request.resuming || calibrating.seen,
        }),
        problem: action.problem,
      };
    }
    case 'named':
      return calibrating
        ? { ...change({ name: action.name }), problem: '' }
        : state;
    case 'confirmAsked':
      return calibrating?.closing === null && !calibrating.confirming
        ? change({ confirming: true })
        : state;
    case 'kept':
      return calibrating?.closing === null && calibrating.confirming
        ? change({ confirming: false })
        : state;
    case 'closingStarted':
      return calibrating?.closing === null
        ? change({ closing: action.how })
        : state;
    case 'closingFailed':
      return calibrating && calibrating.closing !== null
        ? change({ closing: null })
        : state;
    case 'ending':
      return calibrating
        ? { ...state, phase: { kind: 'ending', last: calibrating } }
        : state;
    case 'ended':
      return phase.kind === 'ending'
        ? {
            ...state,
            phase: {
              kind: 'done',
              title: action.title,
              message: action.message,
            },
          }
        : state;
  }
}
