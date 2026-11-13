import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  FindDevices,
  NumberField,
  TextField,
  type EditorProps,
} from '@prysm/web-portal';
import { deviceDefaults, deviceLimits } from './settings';

export function DeviceEditor({ value, onChange }: EditorProps<DeviceSettings>) {
  const { width, height, fps } = deviceLimits;
  return (
    <>
      <FindDevices
        section="capture"
        kind="device"
        path="capture.path"
        field="path"
        label="Find devices"
        looking="Looking for devices…"
        nothing="No devices found."
        fallback="Enter the path below."
        onPick={(path) => onChange({ ...value, path })}
      />
      <TextField
        path="capture.path"
        label="Capture device"
        required
        placeholder="/dev/video0"
        value={value.path}
        onChange={(path) => onChange({ ...value, path })}
        help="Pick it with Find devices, or type its name, such as /dev/video0."
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
