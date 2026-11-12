import type { WebosSettings } from '@prysm/contracts/host-webos';
import {
  CheckboxField,
  NumberField,
  type AdapterUi,
  type EditorProps,
} from '@prysm/web-portal';

/**
 * Limits from the generated `WebosSettings` schema; NV12 needs even sizes
 * (the schema says so in words, so the step of 2 is written here).
 */
export const webosLimits = {
  width: { min: 2, max: 1920, step: 2 },
  height: { min: 2, max: 1080, step: 2 },
  fps: { min: 1, max: 60, step: 1 },
} as const;
/** What Rust uses for an omitted field. */
export const webosDefaults = {
  width: 640,
  height: 360,
  fps: 30,
  include_ui: true,
} as const;

/** Edits the TV capture size, rate and menu blending. */
function WebosEditor({ value, onChange }: EditorProps<WebosSettings>) {
  return (
    <>
      <div className="grid three">
        <NumberField
          path="capture.width"
          label="Width"
          unit="pixels"
          value={value.width ?? webosDefaults.width}
          onChange={(width) => onChange({ ...value, width })}
          {...webosLimits.width}
        />
        <NumberField
          path="capture.height"
          label="Height"
          unit="pixels"
          value={value.height ?? webosDefaults.height}
          onChange={(height) => onChange({ ...value, height })}
          {...webosLimits.height}
        />
        <NumberField
          path="capture.fps"
          label="Frame rate"
          unit="fps"
          value={value.fps ?? webosDefaults.fps}
          onChange={(fps) => onChange({ ...value, fps })}
          {...webosLimits.fps}
        />
      </div>
      <CheckboxField
        path="capture.include_ui"
        label="Include TV menus"
        help="Blend on-screen menus into the capture instead of video only."
        checked={value.include_ui ?? webosDefaults.include_ui}
        onChange={(include_ui) => onChange({ ...value, include_ui })}
      />
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
};
