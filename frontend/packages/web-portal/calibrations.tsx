import { useEffect } from 'react';
import type {
  Calibration,
  CorrectionControls,
  Processing,
} from '@prysm/contracts/application';
import { CheckboxField, FieldMessage, NameField, RangeField } from './fields';
import { FieldScopeProvider } from './form';
import { useLiveProcessing, withCalibrations } from './live-processing';
import {
  correctionLimits,
  corrections,
  noCorrection,
  calibrationLimits,
  primaries,
  primaryLimits,
  signed,
  endsLabel,
  strengthEnds,
} from './calibration-model';
import { Group, RowLink, RowSwitch } from './rows';
import { useSaveReport } from './save-state';

/** How long a deleted calibration can be brought back. */
export const undoForMs = 5000;

/** A calibration deleted from its page: kept in the list's place, to undo for a while. */
export interface DeletedCalibration {
  calibration: Calibration;
  index: number;
  wasActive: boolean;
  undoable: boolean;
}

interface LiveProps {
  processing: Processing;
  revision: number;
  disabled: boolean;
  /** Save the whole processing section against `expected`. */
  save: (expected: number, processing: Processing) => Promise<number>;
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
}

/** The processing section saved live, reported to the page's save status. */
function useCalibrations(props: LiveProps) {
  const live = useLiveProcessing(props);
  useSaveReport('calibrations', live.state);
  return live;
}

/**
 * The calibrations as rows: each opens its own page; one deleted there
 * keeps its place here with Undo for a few seconds, so nothing moves. A new
 * one comes from the Calibrate page.
 */
export function CalibrationList({
  deleted,
  setDeleted,
  open,
  calibrate,
  ...props
}: LiveProps & {
  /** Deleted on this visit, by id; kept by the portal across pages. */
  deleted: Record<string, DeletedCalibration>;
  setDeleted: (
    change: (
      deleted: Record<string, DeletedCalibration>,
    ) => Record<string, DeletedCalibration>,
  ) => void;
  /** Open a calibration's page. */
  open: (id: string) => void;
  /** Make a new calibration on the Calibrate page. */
  calibrate: () => void;
}) {
  const live = useCalibrations(props);
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
  // Undo is offered for a while; the row then just says it was deleted.
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
                        disabled={props.disabled || full}
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
                onClick={() => open(row.calibration.id)}
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
            if (!full && !props.disabled) calibrate();
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
  id,
  deleted,
  leave,
  ...props
}: LiveProps & {
  id: string;
  /** It was deleted: the list shows it in its place, with Undo. */
  deleted: (gone: DeletedCalibration) => void;
  /** Back to the list. */
  leave: () => void;
}) {
  const live = useCalibrations(props);
  const calibrations = live.value.calibrations ?? [];
  const active = live.value.active_calibration;
  const index = calibrations.findIndex((calibration) => calibration.id === id);
  const calibration = calibrations[index];
  if (!calibration)
    return (
      <p className="page-intro">
        This calibration is not there any more.{' '}
        <button type="button" className="link" onClick={leave}>
          Back to the calibrations
        </button>
      </p>
    );
  const path = `processing.calibrations[${index}]`;
  const correction = calibration.correction;
  const inUse = active === id;
  function update(change: Partial<Calibration>, at: string) {
    live.slider.slide(
      withCalibrations(
        live.value,
        calibrations.map((item) =>
          item.id === id ? { ...item, ...change } : item,
        ),
        active,
      ),
      at,
    );
  }
  function correct(change: Partial<CorrectionControls>, at: string) {
    update({ correction: { ...correction, ...change } }, at);
  }
  const shown = calibration;
  function remove() {
    deleted({ calibration: shown, index, wasActive: inUse, undoable: true });
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
  const slider = (name: string) => ({
    path: `${path}.correction.${name}`,
    onPress: live.slider.press,
    onRelease: live.slider.release,
    format: signed,
  });
  return (
    <FieldScopeProvider value={live.scope}>
      <div className="calibration-page" key={live.generation}>
        <NameField
          path={`${path}.name`}
          label="Name"
          help="Saved when you leave the field or press Enter."
          value={calibration.name ?? ''}
          maxChars={calibrationLimits.name}
          onChange={(name) => update({ name }, `${path}.name`)}
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
          {corrections.map(([name, label, help]) => (
            <RangeField
              key={name}
              label={label}
              help={help}
              value={correction[name]}
              onChange={(value) =>
                correct({ [name]: value }, `${path}.correction.${name}`)
              }
              {...slider(name)}
              {...correctionLimits[name]}
            />
          ))}
        </div>
        {primaries.map((primary) => (
          <fieldset key={primary.name}>
            <legend>{primary.label}</legend>
            <div className="grid">
              {(
                [
                  ['hue', primary.hue],
                  ['strength', strengthEnds],
                ] as const
              ).map(([control, ends]) => (
                <RangeField
                  key={control}
                  label={endsLabel(ends)}
                  ends={ends}
                  value={correction[primary.name][control]}
                  onChange={(value) =>
                    correct(
                      {
                        [primary.name]: {
                          ...correction[primary.name],
                          [control]: value,
                        },
                      },
                      `${path}.correction.${primary.name}.${control}`,
                    )
                  }
                  {...slider(`${primary.name}.${control}`)}
                  {...primaryLimits[control]}
                />
              ))}
            </div>
          </fieldset>
        ))}
        <div className="page-actions">
          <div className="actions">
            <button
              type="button"
              disabled={props.disabled}
              onClick={() =>
                update({ correction: noCorrection }, `${path}.correction`)
              }
            >
              Reset to no correction
            </button>
            {/* Destructive, so last and quiet until pointed at. */}
            <button
              type="button"
              className="danger quiet"
              disabled={props.disabled}
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
