import { useReducer, useState } from 'react';
import { usePortal } from './portal-context';
import {
  initialWizard,
  losesWork,
  placeAfter,
  screens,
  wizardReducer,
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
  editCalibration,
  throttled,
  useProcessingEditor,
  withCalibrations,
} from './live-processing';

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
export function useCalibration(leave: () => void) {
  const { snapshot, commands } = usePortal();
  const {
    calibration: shown,
    settings: { processing },
  } = snapshot;
  const { setCalibration } = commands;
  const live = useProcessingEditor();
  const calibrations = live.value.calibrations ?? [];
  const [state, dispatch] = useReducer(wizardReducer, initialWizard);
  const { phase } = state;
  const view = phase.kind === 'ending' ? phase.last : phase;
  const calibrating = phase.kind === 'calibrating' ? phase : null;
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
  const [patterns] = useState(() => throttled<Place>(0));

  const step = view.kind === 'calibrating' ? view : null;
  const editor = editCalibration(live, step?.draftId);
  const draft = editor?.calibration;
  const typedName = (step?.name ?? draft?.name ?? '').trim();

  /** Go to `place` at once; the TV shows its pattern after the request on its way. */
  function request(move: Move, resume = false) {
    if (!calibrating) return;
    const wanted = placeAfter(calibrating.place, move);
    dispatch({ type: 'requestSent', place: wanted, resume });
    patterns.push(wanted, send);
  }
  async function send(wanted: Place) {
    try {
      await setCalibration({
        pattern: screens[wanted.screen]!.pattern,
        uncorrected: wanted.uncorrected,
      });
      dispatch({ type: 'patternShown', place: wanted });
    } catch (error) {
      dispatch({
        type: 'requestFailed',
        place: wanted,
        problem: tvProblem(error),
      });
    }
  }

  async function start() {
    if (phase.kind !== 'start') return;
    // A failed start's save may still land: this start saves the same draft.
    const { from, unconfirmed } = phase;
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
        unconfirmed,
      });
      return;
    }
    // As the settings are now: another device may have saved meanwhile.
    const value = live.current();
    const existing = value.calibrations ?? [];
    const source = existing.find((calibration) => calibration.id === from);
    const id =
      unconfirmed?.draftId ??
      newCalibrationId(existing.map((calibration) => calibration.id));
    // The failed start's save landed meanwhile: its draft is this one.
    const landed = existing.find((calibration) => calibration.id === id);
    const calibration = landed ?? {
      id,
      name: freeName(
        source ? `${source.name} calibrated` : 'Calibrated',
        existing,
      ),
      correction: source ? source.correction : noCorrection,
    };
    const created = withCalibrations(
      value,
      landed ? existing : [...existing, calibration],
      id,
    );
    const started = {
      draftId: id,
      previous: unconfirmed ? unconfirmed.previous : value.active_calibration,
    };
    if (await live.commit(created)) {
      dispatch({
        type: 'started',
        ...started,
        startedWith: calibration.correction,
      });
      return;
    }
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
    await patterns.settle();
    try {
      await setCalibration(null);
    } catch {
      // The engine ends it by itself after ten minutes.
    }
    dispatch({ type: 'ended', title, message });
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
    problem: state.problem,
    target: snapshot.target,
    scope: live.scope,
    generation: live.generation,
    failure: live.failure,
    calibrations,
    view,
    editor,
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
    go: (screen: number, returnTo?: number) => request({ screen, returnTo }),
    /** Show the pattern without correction, or with it again. */
    compare: () => {
      if (!calibrating) return;
      const { place } = calibrating;
      request({
        uncorrected: !place.uncorrected,
      });
    },
    /** Ask the TV to show the patterns again after they ended. */
    resume: () => request({}, true),
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
