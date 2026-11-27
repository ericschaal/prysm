import { useEffect } from 'react';
import type { Calibration } from '@prysm/contracts/application';
import { WhiteSliders, PrimarySliders } from './correction-sliders';
import { CheckboxField, FieldMessage, NameField } from './fields';
import { FieldScopeProvider } from './form';
import {
  editCalibration,
  type ProcessingEditor,
  withCalibrations,
} from './live-processing';
import {
  noCorrection,
  calibrationLimits,
  primaries,
} from './calibration-model';
import { useNav, usePortal } from './portal-context';
import { Group, RowLink, RowSwitch } from './rows';

/** How long a deleted calibration can be brought back. */
export const undoForMs = 5000;

/** A calibration deleted from its page: kept in the list's place, to undo for a while. */
export interface DeletedCalibration {
  calibration: Calibration;
  index: number;
  wasActive: boolean;
  undoable: boolean;
}

/**
 * The calibrations as rows: each opens its own page; one deleted there
 * keeps its place here with Undo for a few seconds, so nothing moves. A new
 * one comes from the Calibrate page.
 */
export function CalibrationList({ live }: { live: ProcessingEditor }) {
  const { deletedCalibrations: deleted, setDeletedCalibrations: setDeleted } =
    usePortal();
  const { go } = useNav();
  const calibrations = live.value.calibrations ?? [];
  const active = live.value.active_calibration;
  const full = calibrations.length >= calibrationLimits.count;

  function undo(gone: DeletedCalibration) {
    setDeleted((current) =>
      Object.fromEntries(
        Object.entries(current).filter(([id]) => id !== gone.calibration.id),
      ),
    );
    const next = [...calibrations];
    next.splice(Math.min(gone.index, next.length), 0, gone.calibration);
    void live.commit(
      withCalibrations(
        live.value,
        next,
        gone.wasActive ? gone.calibration.id : active,
      ),
      'processing.calibrations',
    );
  }
  const undoable = Object.values(deleted).filter((gone) => gone.undoable);
  useEffect(() => {
    if (!undoable.length) return;
    const timer = setTimeout(
      () =>
        setDeleted((current) =>
          Object.fromEntries(
            Object.entries(current).map(([id, gone]) => [
              id,
              { ...gone, undoable: false },
            ]),
          ),
        ),
      undoForMs,
    );
    return () => clearTimeout(timer);
  }, [undoable.length, setDeleted]);

  // The rows: the calibrations, with deleted ones back in their places.
  const rows: ({ calibration: Calibration } | { gone: DeletedCalibration })[] =
    calibrations.map((calibration) => ({ calibration }));
  for (const gone of Object.values(deleted).sort((a, b) => a.index - b.index))
    rows.splice(Math.min(gone.index, rows.length), 0, { gone });

  return (
    <FieldScopeProvider value={live.scope}>
      <p className="page-intro">
        A calibration matches the lights’ colours to your screen and wall. Make
        one per source, such as a games console, and choose the one in use on
        Home.
      </p>
      {rows.length ? (
        <Group title="Saved">
          {rows.map((row) =>
            'gone' in row ? (
              <li
                className="row row-fact"
                key={`gone-${row.gone.calibration.id}`}
              >
                <span className="row-text">
                  <span className="row-label">{row.gone.calibration.name}</span>
                  <span className="row-value" role="status">
                    Deleted.{' '}
                    {row.gone.undoable ? (
                      <button
                        type="button"
                        className="link"
                        disabled={live.scope.disabled || full}
                        onClick={() => undo(row.gone)}
                      >
                        Undo
                      </button>
                    ) : null}
                  </span>
                </span>
              </li>
            ) : (
              <RowLink
                key={row.calibration.id}
                label={row.calibration.name}
                value={row.calibration.id === active ? 'In use' : 'Not in use'}
                onClick={() => go({ calibration: row.calibration.id })}
              />
            ),
          )}
        </Group>
      ) : null}
      <Group title="New">
        <RowLink
          label="New calibration"
          value={
            full
              ? `You have the most Prysm keeps (${calibrationLimits.count}). Delete one first.`
              : 'Test patterns on the TV, step by step'
          }
          onClick={() => {
            if (!full && !live.scope.disabled) go('calibrate');
          }}
        />
      </Group>
      <FieldMessage path="processing.calibrations" touched />
    </FieldScopeProvider>
  );
}

/**
 * One calibration on its own page: its name, whether it is in use, its white
 * balance, red, green and blue, and Delete (back to the list, with Undo there).
 */
export function CalibrationPage({
  live,
  id,
}: {
  live: ProcessingEditor;
  id: string;
}) {
  const { setDeletedCalibrations } = usePortal();
  const nav = useNav();
  const leave = () => nav.back('calibrations');
  const calibrations = live.value.calibrations ?? [];
  const active = live.value.active_calibration;
  const editor = editCalibration(live, id);
  if (!editor)
    return (
      <p className="page-intro">
        This calibration isn’t there any more.{' '}
        <button type="button" className="link" onClick={leave}>
          Back to the calibrations
        </button>
      </p>
    );
  const { calibration, index, path, sliders, update } = editor;
  const inUse = active === id;
  function remove() {
    setDeletedCalibrations((current) => ({
      ...current,
      [id]: { calibration, index, wasActive: inUse, undoable: true },
    }));
    void live.commit(
      withCalibrations(
        live.value,
        calibrations.filter((item) => item.id !== id),
        inUse ? undefined : active,
      ),
      'processing.calibrations',
    );
    leave();
  }
  return (
    <FieldScopeProvider value={live.scope}>
      <div className="calibration-page" key={live.generation}>
        <NameField
          path={`${path}.name`}
          label="Name"
          help="Saved when you leave the field or press Enter."
          value={calibration.name ?? ''}
          maxChars={calibrationLimits.name}
          onChange={(name) => update({ name }, 'name')}
        />
        <ul className="rows">
          <RowSwitch>
            <CheckboxField
              path="processing.active_calibration"
              label="In use"
              help="Its correction shows on the lights. Off: no correction."
              checked={inUse}
              onChange={(on) =>
                void live.commit(
                  withCalibrations(
                    live.value,
                    calibrations,
                    on ? id : undefined,
                  ),
                  'processing.active_calibration',
                )
              }
            />
          </RowSwitch>
        </ul>
        <div className="grid">
          <WhiteSliders {...sliders} />
        </div>
        {primaries.map((primary) => (
          <fieldset key={primary.name}>
            <legend>{primary.label}</legend>
            <div className="grid">
              <PrimarySliders {...sliders} primary={primary} />
            </div>
          </fieldset>
        ))}
        <div className="page-actions">
          <div className="actions">
            <button
              type="button"
              disabled={live.scope.disabled}
              onClick={() => update({ correction: noCorrection }, 'correction')}
            >
              Reset to no correction
            </button>
            {/* Destructive, so last and quiet until pointed at. */}
            <button
              type="button"
              className="danger quiet"
              disabled={live.scope.disabled}
              onClick={remove}
            >
              Delete calibration
            </button>
          </div>
        </div>
      </div>
    </FieldScopeProvider>
  );
}
