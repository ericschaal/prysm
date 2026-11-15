import type { WebosSettings } from '@prysm/contracts/host-webos';
import type { ReactNode } from 'react';
import {
  Advanced,
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
 * (seen on the TV), so Smooth is offered. “Include menus” and `advanced`
 * (the fields at `advancedPaths`) are under Advanced.
 */
export function TvPictureFields<V extends TvPicture>({
  value,
  defaults,
  onChange,
  sizeHint = largeSizeHint,
  advanced,
  advancedPaths = [],
}: EditorProps<V> & {
  defaults: Required<TvPicture>;
  /** Help shown under Custom while the size is above 640 × 360. */
  sizeHint?: string;
  advanced?: ReactNode;
  advancedPaths?: readonly string[];
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
      <Advanced paths={['capture.include_ui', ...advancedPaths]}>
        <CheckboxField
          path="capture.include_ui"
          label="Include TV menus"
          help={includeMenusHelp}
          checked={value.include_ui ?? defaults.include_ui}
          onChange={(include_ui) => onChange({ ...value, include_ui })}
        />
        {advanced}
      </Advanced>
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
