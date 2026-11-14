import type { WebosSettings } from '@prysm/contracts/host-webos';
import {
  allPicturePresets,
  CheckboxField,
  PictureChoice,
  type EditorProps,
} from '@prysm/web-portal';
import {
  includeMenusHelp,
  largeSizeHint,
  enoughPixels,
  tvPictureLimits,
  webosDefaults,
  type TvPicture,
} from './settings';

/**
 * Size, rate and “include menus” of a TV's picture, shared by this TV's
 * capture and a capture-only TV's stream request. `defaults` are what Rust
 * uses for an omitted field of that section. Both can capture 60 fps
 * (seen on the TV), so Smooth is offered.
 */
export function TvPictureFields<V extends TvPicture>({
  value,
  defaults,
  onChange,
  sizeHint = largeSizeHint,
}: EditorProps<V> & {
  defaults: Required<TvPicture>;
  /** Help shown under Custom while the size is above 640 × 360. */
  sizeHint?: string;
}) {
  const large =
    (value.width ?? defaults.width) * (value.height ?? defaults.height) >
    enoughPixels;
  return (
    <>
      <PictureChoice
        value={value}
        defaults={defaults}
        onChange={onChange}
        presets={allPicturePresets}
        limits={tvPictureLimits}
        customNote={
          large ? (
            <p className="help" data-size-hint>
              {sizeHint}
            </p>
          ) : null
        }
      />
      <CheckboxField
        path="capture.include_ui"
        label="Include TV menus"
        help={includeMenusHelp}
        checked={value.include_ui ?? defaults.include_ui}
        onChange={(include_ui) => onChange({ ...value, include_ui })}
      />
    </>
  );
}

export function WebosEditor(props: EditorProps<WebosSettings>) {
  return (
    <>
      <TvPictureFields {...props} defaults={webosDefaults} />
      <p className="help">Video comes directly from this TV.</p>
    </>
  );
}
