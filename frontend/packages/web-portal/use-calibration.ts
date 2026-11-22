import { useReducer, useRef, useState } from 'react';
import type {
  CalibrationPattern,
  TargetReading,
  Pattern,
} from '@prysm/application-client';
import type {
  CorrectionControls,
  Processing,
} from '@prysm/contracts/application';
import {
  initialWizard,
  losesWork,
  placeAfter,
  screens,
  wizardReducer,
  type Calibrating,
  type Move,
  type Place,
} from './calibrate-wizard';
import { tvProblem } from './calibrate-copy';
import { nameProblem } from './checks';
import {
  calibrationLimits,
  freeName,
  newCalibrationId,
  noCorrection,
} from './calibration-model';
import {
  throttled,
  useLiveProcessing,
  withCalibrations,
} from './live-processing';

export interface CalibrateProps {
  processing: Processing;
  revision: number;
  target: TargetReading;
  /** The pattern the TV shows, absent while not calibrating. */
  shown: CalibrationPattern | undefined;
  disabled: boolean;
  save: (expected: number, processing: Processing) => Promise<number>;
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
  setCalibration: (pattern: CalibrationPattern | null) => Promise<unknown>;
  /** Go back to the portal's main page. */
  leave: () => void;
}

/** A pattern asked of the TV, and the wizard's place with it. */
interface Wanted {
  pattern: Pattern;
  place: Place;
}

/**
 * The Calibrate page's work: the wizard's state, the TV's patterns and the
 * draft calibration, saved live. Every tap is an action here; the page only
 * renders what this returns.
 *
 * Start asks the TV for the first pattern, then saves the draft calibration
 * (new, or a copy) as the active one; if that save fails, the draft is put
 * back and the TV's pattern ended, so nothing of the start stays. A save
 * whose answer never came may have run all the same: the wizard remembers
 * the draft, and once the state shows it, calibration goes on (ended on the
 * TV, with Resume). Finish names the draft, Cancel deletes it and makes the
 * previous calibration active again; each waits for the saves before it.
 */
export function useCalibration({
  processing,
  revision,
  shown,
  disabled,
  save,
  tryProcessing,
  setCalibration,
  leave,
}: CalibrateProps) {
  const live = useLiveProcessing({
    processing,
    revision,
    disabled,
    save,
    tryProcessing,
  });
  const calibrations = live.value.calibrations ?? [];
  const [state, dispatch] = useReducer(wizardReducer, initialWizard);
  const { phase } = state;
  const calibrating = phase.kind === 'calibrating' ? phase : null;
  /** The screen shown: also while the TV's calibration is being ended. */
  const view: Calibrating | null =
    phase.kind === 'ending' ? phase.last : calibrating;
  if (shown && calibrating && !calibrating.seen)
    dispatch({ type: 'patternSeen' });
  const unconfirmed = phase.kind === 'start' ? phase.unconfirmed : null;
  /** The draft of a failed start, as the state shows it: its save ran. */
  const confirmed =
    unconfirmed &&
    processing.calibrations?.find(
      (calibration) => calibration.id === unconfirmed.draftId,
    );
  if (confirmed)
    dispatch({ type: 'startConfirmed', startedWith: confirmed.correction });
  /** The TV's patterns: one request at a time, the newest waiting one next. */
  const [patterns] = useState(() => throttled<Wanted>(0));
  /** The pattern asked for last; an older one's answer no longer decides. */
  const newestRef = useRef<Wanted | null>(null);

  const draftIndex = view
    ? calibrations.findIndex((calibration) => calibration.id === view.draftId)
    : -1;
  const draft = draftIndex >= 0 ? calibrations[draftIndex] : undefined;
  /** The draft's correction in the processing section. */
  const path = `processing.calibrations[${draftIndex}].correction`;
  const typedName = (view?.name ?? draft?.name ?? '').trim();

  /** Go to `place` at once; the TV shows `pattern` after the request on its way. */
  function request(pattern: Pattern, move: Move, resume = false) {
    if (!calibrating) return;
    const wanted: Wanted = {
      pattern,
      place: placeAfter(calibrating.place, move),
    };
    newestRef.current = wanted;
    dispatch({ type: 'requestSent', place: wanted.place, resume });
    patterns.push(wanted, send);
  }
  async function send(wanted: Wanted) {
    try {
      await setCalibration({
        pattern: wanted.pattern,
        uncorrected: wanted.place.uncorrected,
      });
      dispatch(
        newestRef.current === wanted
          ? { type: 'patternShown' }
          : { type: 'patternShown', more: wanted.place },
      );
    } catch (error) {
      // A newer request decides what the TV shows.
      if (newestRef.current === wanted)
        dispatch({ type: 'requestFailed', problem: tvProblem(error) });
    }
  }

  async function start() {
    if (phase.kind !== 'start') return;
    const { from } = phase;
    dispatch({ type: 'startRequested' });
    try {
      await setCalibration({
        pattern: screens[0]!.pattern,
        uncorrected: false,
      });
    } catch (error) {
      dispatch({
        type: 'startFailed',
        problem: tvProblem(error),
        unconfirmed: null,
      });
      return;
    }
    // As the settings are now: another device may have saved meanwhile.
    const value = live.current();
    const existing = value.calibrations ?? [];
    const source = existing.find((calibration) => calibration.id === from);
    const id = newCalibrationId(existing.map((calibration) => calibration.id));
    const calibration = {
      id,
      name: freeName(
        source ? `${source.name} calibrated` : 'Calibrated',
        existing,
      ),
      correction: source ? source.correction : noCorrection,
    };
    const created = withCalibrations(value, [...existing, calibration], id);
    const started = { draftId: id, previous: value.active_calibration };
    if (await live.commit(created)) {
      dispatch({
        type: 'started',
        ...started,
        startedWith: calibration.correction,
      });
      return;
    }
    // Not saved (the failure says why): nothing of the start may stay, in
    // the draft, which would save itself later, or on the TV. A save whose
    // answer never came may have run: the state shows it if it did.
    live.reload();
    try {
      await setCalibration(null);
    } catch {
      // The engine ends it by itself after ten minutes.
    }
    dispatch({ type: 'startFailed', problem: '', unconfirmed: started });
  }

  async function end(title: string, message: string) {
    dispatch({ type: 'ending' });
    // A pattern still on its way must not show after the end.
    newestRef.current = null;
    await patterns.settle();
    try {
      await setCalibration(null);
    } catch {
      // The engine ends it by itself after ten minutes.
    }
    dispatch({ type: 'ended', title, message });
  }

  /** The draft's correction changed by `change`; `control` is where a failed save shows. */
  function correct(change: Partial<CorrectionControls>, control: string) {
    if (!draft) return;
    const next = {
      ...draft,
      correction: { ...draft.correction, ...change },
    };
    // The lights follow a held slider at once; its release saves.
    live.slider.slide(
      withCalibrations(
        live.value,
        calibrations.map((calibration, at) =>
          at === draftIndex ? next : calibration,
        ),
        live.value.active_calibration,
      ),
      `${path}.${control}`,
    );
  }

  async function finish() {
    if (!calibrating || calibrating.closing || !draft) return;
    dispatch({ type: 'closingStarted', how: 'finish' });
    const others = calibrations.filter(
      (calibration) => calibration.id !== draft.id,
    );
    const final = freeName(typedName, others);
    // A change still waiting for its delay, or on its way, goes first.
    if (final !== draft.name || live.dirty) {
      const renamed = calibrations.map((calibration) =>
        calibration.id === draft.id
          ? { ...calibration, name: final }
          : calibration,
      );
      const value = withCalibrations(
        live.value,
        renamed,
        live.value.active_calibration,
      );
      const saved = await live.commit(value);
      // Not saved, or deleted elsewhere meanwhile (the page then says so):
      // nothing is finished.
      const kept = saved?.calibrations?.some(
        (calibration) => calibration.id === draft.id,
      );
      if (!kept) {
        dispatch({ type: 'closingFailed' });
        return;
      }
    }
    await end('Calibration finished', `“${final}” is saved and active.`);
  }

  async function discard() {
    if (!calibrating || calibrating.closing) return;
    dispatch({ type: 'closingStarted', how: 'discard' });
    const rest = calibrations.filter(
      (calibration) => calibration.id !== calibrating.draftId,
    );
    const back = rest.some(
      (calibration) => calibration.id === calibrating.previous,
    )
      ? calibrating.previous
      : undefined;
    if (!(await live.commit(withCalibrations(live.value, rest, back)))) {
      dispatch({ type: 'closingFailed' });
      return;
    }
    await end(
      'Calibration cancelled',
      'The new calibration was deleted; your colours are as before.',
    );
  }

  return {
    state,
    live,
    calibrations,
    /** The calibrating screen, also shown while calibration is being ended. */
    view,
    draft,
    path,
    /** The name Finish saves, before it is made unique. */
    typedName,
    nameProblem: nameProblem(typedName, calibrationLimits.name),
    /** The TV no longer shows the patterns (its timeout, or it left them). */
    ended:
      !!calibrating &&
      calibrating.seen &&
      !shown &&
      calibrating.request.kind === 'idle',
    /** The pattern is on its way to the TV, or calibration is being ended. */
    pending: phase.kind === 'ending' || calibrating?.request.kind === 'sending',
    choose: (from: string) => dispatch({ type: 'sourceChosen', from }),
    start,
    /** Go to `screen`, opened from the check `returnTo` if given. */
    go: (screen: number, returnTo?: number) =>
      request(screens[screen]!.pattern, { screen, returnTo }),
    /** Show the pattern without correction, or with it again. */
    compare: () => {
      if (!calibrating) return;
      const { place } = calibrating;
      request(screens[place.screen]!.pattern, {
        uncorrected: !place.uncorrected,
      });
    },
    /** Ask the TV to show the patterns again after they ended. */
    resume: () => {
      if (calibrating)
        request(screens[calibrating.place.screen]!.pattern, {}, true);
    },
    correct,
    name: (name: string) => dispatch({ type: 'named', name }),
    finish,
    /** Cancel: with changes it asks first, else it discards at once. */
    cancel: () => {
      if (!calibrating || calibrating.closing) return;
      if (losesWork(calibrating, draft)) dispatch({ type: 'confirmAsked' });
      else void discard();
    },
    keep: () => dispatch({ type: 'kept' }),
    discard,
    /** Leave once a change still waiting is saved; calibration stays ended. */
    leave: async () => {
      if (!calibrating || calibrating.closing) return;
      dispatch({ type: 'closingStarted', how: 'leave' });
      if (live.dirty && !(await live.commit(live.value))) {
        dispatch({ type: 'closingFailed' });
        return;
      }
      leave();
    },
  };
}

export type CalibrationView = ReturnType<typeof useCalibration>;
