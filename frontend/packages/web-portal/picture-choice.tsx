import { use, useId, useState, type ReactNode } from 'react';
import { pictureFacts, type Picture } from './adapters';
import type { NumberLimits } from './checks';
import { NumberField } from './fields';
import { FieldContext } from './form';
import {
  customPicture,
  picturePaths,
  picturePresets,
  presetOf,
  type PicturePreset,
} from './picture-presets';
import { RowChoice } from './rows';

/**
 * How much work capture costs, as one choice: a few presets and Custom,
 * in the rows pattern. Nothing new is stored: the ticked row follows from
 * the saved size and rate. Custom opens the three number fields below the
 * list, so nothing above them moves, and stays open while the user types
 * there, even through numbers that match a preset.
 */
export function PictureChoice<V extends Picture>({
  value,
  defaults,
  onChange,
  presets,
  limits,
  rate = { label: 'Frame rate' },
  customNote,
}: {
  value: V;
  /** What Rust uses for an omitted field of this section. */
  defaults: Required<Picture>;
  onChange: (next: V) => void;
  /** The presets this source can do, in the order of `allPicturePresets`. */
  presets: readonly PicturePreset[];
  limits: Record<'width' | 'height' | 'fps', NumberLimits>;
  /** The rate field's label and help, where the source says more. */
  rate?: { label: string; help?: string };
  /** Shown under the Custom fields, such as the large-size hint. */
  customNote?: ReactNode;
}) {
  const scope = use(FieldContext);
  const nameId = useId();
  const legendId = useId();
  const preset = presetOf(value, defaults, presets);
  // A refused size or rate must be seen where it can be corrected.
  const refused = picturePaths.some((path) => scope.errors[path]);
  const [customOpen, setCustomOpen] = useState(preset === null);
  const custom = customOpen || preset === null || refused;
  function choose(next: PicturePreset) {
    const { width, height, fps } = picturePresets[next];
    for (const path of picturePaths) scope.edited(path);
    setCustomOpen(false);
    onChange({ ...value, width, height, fps });
  }
  return (
    <>
      <fieldset
        className="picture-choice"
        role="radiogroup"
        aria-labelledby={legendId}
      >
        <legend id={legendId}>Size and rate</legend>
        <ul className="rows">
          {presets.map((key) => {
            const at = picturePresets[key];
            return (
              <RowChoice
                key={key}
                name={nameId}
                label={at.label}
                about={at.about}
                facts={pictureFacts(at, at)
                  .map((fact) => fact.value)
                  .join(' · ')}
                checked={!custom && preset === key}
                disabled={scope.disabled}
                onChoose={() => choose(key)}
              />
            );
          })}
          <RowChoice
            name={nameId}
            label={customPicture.label}
            about={customPicture.about}
            checked={custom}
            disabled={scope.disabled}
            onChoose={() => setCustomOpen(true)}
          />
        </ul>
      </fieldset>
      {custom ? (
        <div className="picture-custom">
          <div className="grid three">
            <NumberField
              path="capture.width"
              label="Width"
              unit="pixels"
              value={value.width ?? defaults.width}
              onChange={(width) => onChange({ ...value, width })}
              {...limits.width}
            />
            <NumberField
              path="capture.height"
              label="Height"
              unit="pixels"
              value={value.height ?? defaults.height}
              onChange={(height) => onChange({ ...value, height })}
              {...limits.height}
            />
            <NumberField
              path="capture.fps"
              label={rate.label}
              unit="fps"
              value={value.fps ?? defaults.fps}
              onChange={(fps) => onChange({ ...value, fps })}
              help={rate.help}
              {...limits.fps}
            />
          </div>
          {customNote}
        </div>
      ) : null}
    </>
  );
}
