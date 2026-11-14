/**
 * The Calibrate page's state: which screen shows, the TV request in flight
 * and the problem to show. Pure, so the transitions are tested on their own.
 */
import type { Pattern } from '@prysm/application-client';
import type { HueControls } from '@prysm/contracts/application';

export type Section = 'white' | 'hues' | 'intensity' | 'mixed' | 'review';
export type Hue = keyof HueControls;

export interface Screen {
  section: Section;
  pattern: Pattern;
  /** The hue this screen adjusts (hue screens only). */
  hue?: Hue;
}

/** The wizard, one TV pattern per screen. */
export const screens: readonly Screen[] = [
  { section: 'white', pattern: 'gray-192' },
  { section: 'hues', pattern: 'red', hue: 'red' },
  { section: 'hues', pattern: 'yellow', hue: 'yellow' },
  { section: 'hues', pattern: 'green', hue: 'green' },
  { section: 'hues', pattern: 'cyan', hue: 'cyan' },
  { section: 'hues', pattern: 'blue-pair', hue: 'blue' },
  { section: 'hues', pattern: 'magenta', hue: 'magenta' },
  { section: 'intensity', pattern: 'red-pair' },
  { section: 'intensity', pattern: 'green-pair' },
  { section: 'intensity', pattern: 'blue-pair' },
  { section: 'mixed', pattern: 'coast' },
  { section: 'mixed', pattern: 'garden' },
  { section: 'mixed', pattern: 'dusk' },
  { section: 'review', pattern: 'review-grid' },
];

export type WizardPhase =
  /** Before the start: the profile to copy (`''` for a new one). */
  | { kind: 'start'; from: string }
  | {
      kind: 'calibrating';
      /** The draft profile being calibrated. */
      draftId: string;
      /** The active profile before the start, for Cancel. */
      previous: string | undefined;
      /** Index into {@link screens}. */
      screen: number;
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
  /** The TV shows the pattern of `screen` (the current one when absent). */
  | { type: 'patternShown'; screen?: number }
  | { type: 'requestFailed'; problem: string }
  | { type: 'started'; draftId: string; previous: string | undefined }
  | { type: 'named'; name: string }
  | { type: 'refused'; problem: string }
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
            ? { ...phase, screen: action.screen }
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
          name: null,
        },
      };
    case 'named':
      return phase.kind === 'calibrating'
        ? { ...state, problem: '', phase: { ...phase, name: action.name } }
        : state;
    case 'refused':
      return { ...state, problem: action.problem };
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
