import { use, useEffect, useId, useState } from 'react';
import { describePicture, pictureFacts, type Picture } from './adapters';
import {
  delivers,
  offeredPresets,
  sameSize,
  type ModesState,
  type Offered,
} from './capture-modes';
import { FieldContext, shownError } from './form';
import {
  customPicture,
  picturePaths,
  picturePresets,
  presetOrder,
  type PicturePreset,
} from './picture-presets';
import { RowChoice } from './rows';

/**
 * How much work capture costs, as one choice of presets in the rows
 * pattern, each at the size and rate the source delivers for it; only the
 * ticked row says what it means. Nothing new is stored: the ticked row
 * follows from the size and rate. One more ticked row shows a size and rate
 * no preset sets, until a preset replaces it. Until the modes are known the
 * rows are locked on the current choice, with `unreachable` said while the
 * hardware is out of reach. A preset ticked earlier follows to this source's
 * size for it; else a size and rate the source cannot deliver moves to the
 * first preset. Either way, what could not be delivered is said in the line
 * above the rows. Nothing is drawn when
 * `nothingOffered`: the field that selects the source says why.
 */
export function PictureChoice<V extends Picture>({
  value,
  defaults,
  onChange,
  modes,
  source,
  unreachable,
  help,
}: {
  value: V;
  /** What Rust uses for an omitted field of this section. */
  defaults: Required<Picture>;
  onChange: (next: V) => void;
  modes: ModesState;
  /** What delivers the picture, to start a sentence: `This device`. */
  source: string;
  /** Why nothing can be chosen while the hardware is out of reach. */
  unreachable: string;
  /** Said in the line under the rows while it has nothing else to say. */
  help?: string;
}) {
  const scope = use(FieldContext);
  const nameId = useId();
  const legendId = useId();
  const lineId = useId();
  const current = {
    width: value.width ?? defaults.width,
    height: value.height ?? defaults.height,
    fps: value.fps ?? defaults.fps,
  };
  const known = modes.state === 'known' ? modes.modes : null;
  const offered: Offered[] = known
    ? offeredPresets(known)
    : presetOrder.map((preset) => ({ preset, ...picturePresets[preset] }));
  const ticked = offered.find((each) => sameSize(each, current));
  /** The preset last ticked while the modes were known. */
  const [intent, setIntent] = useState<PicturePreset | null>(null);
  /** Said once a size and rate the source cannot deliver was replaced. */
  const [moved, setMoved] = useState<string | null>(null);
  if (known && ticked && ticked.preset !== intent) setIntent(ticked.preset);
  // A line about other modes says nothing about these.
  if (!known && moved !== null) setMoved(null);
  const followed = offered.find((each) => each.preset === intent);
  let move: Offered | undefined;
  if (known && !ticked && !scope.disabled)
    move = followed ?? (delivers(known, current) ? undefined : offered[0]);
  // Said whenever what was set cannot be delivered; following a preset to
  // a deliverable size is not.
  if (known && move && !delivers(known, current)) {
    const line = `${source} can’t do ${describePicture(current.width, current.height, current.fps)}, so it is set to ${picturePresets[move.preset].label}.`;
    if (moved !== line) setMoved(line);
  }
  useEffect(() => {
    if (!move) return;
    const { width, height, fps } = move;
    onChange({ ...value, width, height, fps });
  }, [move, value, onChange]);

  if (known && offered.length === 0) return null;
  const locked = scope.disabled || !known;
  const error = picturePaths
    .map((path) => shownError(scope, path, false))
    .find(Boolean);
  let line: string | null = null;
  if (modes.state === 'unknown') line = unreachable;
  else if (modes.state === 'failed')
    line = 'Prysm didn’t say what this source can do. Asking again…';
  else if (known) line = moved;
  line ??= help ?? null;

  function choose({ width, height, fps }: Offered) {
    for (const path of picturePaths) scope.edited(path);
    setMoved(null);
    onChange({ ...value, width, height, fps });
  }
  const facts = (size: Required<Picture>) =>
    pictureFacts(size, size)
      .map((fact) => fact.value)
      .join(' · ');
  return (
    <fieldset
      className="picture-choice"
      role="radiogroup"
      aria-labelledby={legendId}
      aria-describedby={
        (error && `${lineId}-error`) || (line && `${lineId}-help`) || undefined
      }
      aria-invalid={error ? true : undefined}
      data-field={picturePaths.join(' ')}
      tabIndex={-1}
    >
      <legend id={legendId}>Size and rate</legend>
      <ul className="rows">
        {offered.map((each) => {
          const at = picturePresets[each.preset];
          const checked = each === ticked;
          return (
            <RowChoice
              key={each.preset}
              name={nameId}
              label={at.label}
              about={checked ? at.about : undefined}
              facts={facts(each)}
              checked={checked}
              disabled={locked}
              onChoose={() => choose(each)}
            />
          );
        })}
        {ticked || move ? null : (
          <RowChoice
            name={nameId}
            label={customPicture.label}
            about={customPicture.about}
            facts={facts(current)}
            checked
            onChoose={() => {}}
          />
        )}
      </ul>
      {/* Under the rows, and of no height while it has nothing to say. */}
      <div
        className="field-note"
        data-state={error ? 'error' : 'help'}
        data-said={Boolean(error || line)}
        role="status"
      >
        <p className="help" id={`${lineId}-help`}>
          {line}
        </p>
        <p className="field-error" id={`${lineId}-error`}>
          {error}
        </p>
      </div>
    </fieldset>
  );
}
