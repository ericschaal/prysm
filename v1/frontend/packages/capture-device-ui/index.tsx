import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  NumberField,
  TextField,
  type AdapterUi,
  type EditorProps,
} from '@prysm/web-portal';

/**
 * Defaults and limits from the generated `DeviceSettings` schema. `fps` has no
 * schema maximum; it is bounded here by AVFoundation's `i32` timescale, the
 * stricter of the two platforms (the Rust check on macOS).
 */
export const deviceLimits = {
  width: { min: 1, max: 3840, step: 1 },
  height: { min: 1, max: 2160, step: 1 },
  fps: { min: 1, max: 2147483647, step: 1 },
} as const;
/** What Rust uses for an omitted field. */
export const deviceDefaults = { width: 640, height: 360, fps: 30 } as const;

function DeviceEditor({ value, onChange }: EditorProps<DeviceSettings>) {
  const { width, height, fps } = deviceLimits;
  return (
    <>
      <TextField
        path="capture.path"
        label="Capture device"
        required
        placeholder="/dev/video0"
        value={value.path}
        onChange={(path) => onChange({ ...value, path })}
        help="A V4L2 device node on Linux, such as /dev/video0, or the AVFoundation unique ID on macOS. In a container, use the mapped device path."
      />
      <div className="grid three">
        <NumberField
          path="capture.width"
          label="Width"
          unit="pixels"
          value={value.width ?? deviceDefaults.width}
          onChange={(next) => onChange({ ...value, width: next })}
          {...width}
        />
        <NumberField
          path="capture.height"
          label="Height"
          unit="pixels"
          value={value.height ?? deviceDefaults.height}
          onChange={(next) => onChange({ ...value, height: next })}
          {...height}
        />
        <NumberField
          path="capture.fps"
          label="Frame rate"
          unit="fps"
          value={value.fps ?? deviceDefaults.fps}
          onChange={(next) => onChange({ ...value, fps: next })}
          {...fps}
        />
      </div>
      <p className="help">
        Choose a size and rate your device supports; 640 × 360 at 30 fps is
        plenty for lighting.
      </p>
    </>
  );
}

export const deviceCapture: AdapterUi<DeviceSettings> = {
  label: 'Video capture device',
  create: () => ({ kind: 'device', path: '' }),
  Editor: DeviceEditor,
  normalize: (value) => ({ ...value, path: value.path.trim() }),
};
