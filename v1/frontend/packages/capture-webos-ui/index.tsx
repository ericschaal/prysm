import type { WebosSettings } from '@prysm/contracts/host-webos';
import {
  CheckboxField,
  describePicture,
  NumberField,
  type AdapterUi,
  type EditorProps,
} from '@prysm/web-portal';

/**
 * Limits of the picture a TV captures, the same for this TV (`WebosSettings`)
 * and for a capture-only TV (`RemoteReceiverSettings`); NV12 needs even sizes
 * (the schemas say so with `multipleOf`, so the step of 2 is written here).
 */
export const tvPictureLimits = {
  width: { min: 2, max: 1920, step: 2 },
  height: { min: 2, max: 1080, step: 2 },
  fps: { min: 1, max: 60, step: 1 },
} as const;

/** The picture settings a TV capture section has; omitted ones use defaults. */
export interface TvPicture {
  width?: number;
  height?: number;
  fps?: number;
  include_ui?: boolean;
}

/** `640 × 360 at 30 fps, with menus` for a TV picture. */
export function describeTvPicture(value: TvPicture, defaults: Required<TvPicture>) {
  const picture = describePicture(
    value.width ?? defaults.width,
    value.height ?? defaults.height,
    value.fps ?? defaults.fps,
  );
  return `${picture}, ${(value.include_ui ?? defaults.include_ui) ? 'with' : 'without'} menus`;
}

export const includeMenusHelp =
  'Menus are captured over the video, and when nothing is playing (such as the Home screen) the TV’s own screens are captured. Off: video only.';

/**
 * Size, rate and “include menus” of a TV's picture, shared by this TV's
 * capture and a capture-only TV's stream request. `defaults` are what Rust
 * uses for an omitted field of that section.
 */
export function TvPictureFields<V extends TvPicture>({
  value,
  defaults,
  onChange,
}: EditorProps<V> & { defaults: Required<TvPicture> }) {
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

/** What Rust uses for an omitted field. */
export const webosDefaults = {
  width: 640,
  height: 360,
  fps: 30,
  include_ui: true,
} as const;

function WebosEditor(props: EditorProps<WebosSettings>) {
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

export const webosCapture: AdapterUi<WebosSettings> = {
  label: 'This TV',
  create: () => ({ kind: 'webos' }),
  Editor: WebosEditor,
  summary: (value) => describeTvPicture(value, webosDefaults),
};
