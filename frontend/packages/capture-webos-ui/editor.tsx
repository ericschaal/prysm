import type { WebosSettings } from '@prysm/contracts/host-webos';
import {
  CheckboxField,
  NumberField,
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
 * uses for an omitted field of that section.
 */
export function TvPictureFields<V extends TvPicture>({
  value,
  defaults,
  onChange,
  sizeHint = largeSizeHint,
}: EditorProps<V> & {
  defaults: Required<TvPicture>;
  /** Help shown while the size is above 640 × 360. */
  sizeHint?: string;
}) {
  const large =
    (value.width ?? defaults.width) * (value.height ?? defaults.height) >
    enoughPixels;
  return (
    <>
      <div className="grid three">
        <NumberField
          path="capture.width"
          label="Width"
          unit="pixels"
          value={value.width ?? defaults.width}
          onChange={(width) => onChange({ ...value, width })}
          {...tvPictureLimits.width}
        />
        <NumberField
          path="capture.height"
          label="Height"
          unit="pixels"
          value={value.height ?? defaults.height}
          onChange={(height) => onChange({ ...value, height })}
          {...tvPictureLimits.height}
        />
        <NumberField
          path="capture.fps"
          label="Frame rate"
          unit="fps"
          value={value.fps ?? defaults.fps}
          onChange={(fps) => onChange({ ...value, fps })}
          {...tvPictureLimits.fps}
        />
      </div>
      {large ? (
        <p className="help" data-size-hint>
          {sizeHint}
        </p>
      ) : null}
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
      <p className="help">
        Video comes directly from this TV. Start with 640 × 360 at 30 fps; sizes
        must be even and the rate at most 60 fps.
      </p>
    </>
  );
}
