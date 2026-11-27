import { use, useEffect, useId, useRef, useState } from 'react';
import {
  describePicture,
  pictureSize,
  savedPicture,
  type Picture,
} from './adapters';
import {
  delivers,
  sameSize,
  type ModesState,
  type Offered,
} from './capture-modes';
import { FieldContext, shownError } from './form';
import {
  customPicture,
  picturePaths,
  picturePresets,
  presetTargets,
  type PicturePreset,
} from './picture-presets';
import { RowChoice } from './rows';

/**
 * Choose a size and rate from the source's advertised modes. A remembered
 * preset follows the new source. An unsupported custom size falls back to
 * the first offered preset.
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
  const groupRef = useRef<HTMLFieldSetElement>(null);
  /** Where focus was when the user requested another probe. */
  const askedAgainRef = useRef<{ from: Element | null } | null>(null);
  const current = savedPicture(value, defaults);
  const known = modes.state === 'known' ? modes : null;
  const offered = known?.offered ?? presetTargets;
  const ticked = offered.find((each) => sameSize(each, current));
  /** The preset last ticked while the modes were known. */
  const [intent, setIntent] = useState<PicturePreset | null>(null);
  /** Said once a size and rate the source cannot deliver was replaced. */
  const [moved, setMoved] = useState<string | null>(null);
  if (known && ticked && ticked.preset !== intent) setIntent(ticked.preset);
  if (!known && moved !== null) setMoved(null);
  let move: Offered | undefined;
  if (known && !ticked && !scope.disabled) {
    const supported = delivers(known.modes, current);
    move =
      offered.find((each) => each.preset === intent) ??
      (supported ? undefined : offered[0]);
    if (move && !supported) {
      const line = `${source} doesn’t offer ${describePicture(current.width, current.height, current.fps)}. ${picturePresets[move.preset].label} is chosen.`;
      if (moved !== line) setMoved(line);
    }
  }
  useEffect(() => {
    if (!move) return;
    const { width, height, fps } = move;
    onChange({ ...value, width, height, fps });
  }, [move, value, onChange]);
  useEffect(() => {
    const asked = askedAgainRef.current;
    if (!asked) return;
    askedAgainRef.current = null;
    const now = document.activeElement;
    const moved = now && now !== document.body && now !== asked.from;
    if (modes.state === 'known' && !moved) groupRef.current?.focus();
  }, [modes]);

  if (known && offered.length === 0) return null;
  const locked = scope.disabled || !known;
  const error = picturePaths
    .map((path) => shownError(scope, path, false))
    .find(Boolean);
  let line: string | null = null;
  if (modes.state === 'unknown') line = unreachable;
  else if (modes.state === 'failed')
    line = 'Prysm didn’t say what this source can do.';
  else if (known) line = moved;
  line ??= help ?? null;
  const again = 'again' in modes ? modes.again : undefined;

  function choose({ width, height, fps }: Offered) {
    for (const path of picturePaths) scope.edited(path);
    setMoved(null);
    onChange({ ...value, width, height, fps });
  }
  const aboutOf = (each: Offered) => {
    const at = picturePresets[each.preset];
    const object = source.charAt(0).toLowerCase() + source.slice(1);
    return each.width === at.width && each.height === at.height
      ? at.about
      : `The smallest size ${object} offers at ${each.fps} fps.`;
  };
  const facts = ({ width, height, fps }: Required<Picture>) =>
    `${pictureSize(width, height)} · ${fps} fps`;
  return (
    <fieldset
      ref={groupRef}
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
              about={checked ? aboutOf(each) : undefined}
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
          {again ? (
            <>
              {' '}
              <button
                type="button"
                className="link"
                onClick={() => {
                  askedAgainRef.current = { from: document.activeElement };
                  again();
                }}
              >
                Try again
              </button>
            </>
          ) : null}
        </p>
        <p className="field-error" id={`${lineId}-error`}>
          {error}
        </p>
      </div>
    </fieldset>
  );
}
