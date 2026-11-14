import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  FindDevices,
  PictureChoice,
  TextField,
  type EditorProps,
} from '@prysm/web-portal';
import { deviceDefaults, deviceLimits, devicePresets } from './settings';

export function DeviceEditor({ value, onChange }: EditorProps<DeviceSettings>) {
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
      <PictureChoice
        value={value}
        defaults={deviceDefaults}
        onChange={onChange}
        presets={devicePresets}
        limits={deviceLimits}
      />
      <p className="help">
        The device must support the size and rate you choose.
      </p>
    </>
  );
}
