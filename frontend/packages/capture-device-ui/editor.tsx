import { useEffect, useState } from 'react';
import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  DeviceRows,
  nothingOffered,
  PictureChoice,
  TextField,
  useCaptureModes,
  type EditorProps,
} from '@prysm/web-portal';
import { deviceDefaults } from './settings';

/** How long typing pauses before the typed device is asked about. */
export const typedDeviceMs = 400;

/**
 * The device as rows from discovery (typed under Advanced), then its sizes
 * and rates once it is known which device: asked of the device itself.
 */
export function DeviceEditor({ value, onChange }: EditorProps<DeviceSettings>) {
  const path = value.path.trim();
  // A typed path is asked about once typing pauses; a picked one at once.
  const [asked, setAsked] = useState(path);
  useEffect(() => {
    const timer = setTimeout(() => setAsked(path), typedDeviceMs);
    return () => clearTimeout(timer);
  }, [path]);
  const modes = useCaptureModes(asked ? { kind: 'device', path: asked } : null);
  const pick = (next: string) => {
    setAsked(next);
    onChange({ ...value, path: next });
  };
  return (
    <DeviceRows
      kind="device"
      path="capture.path"
      field="path"
      legend="Device"
      value={path}
      missing={modes.state === 'unknown' ? 'Not plugged in' : undefined}
      problem={
        asked && nothingOffered(modes)
          ? 'Prysm can’t capture from this device.'
          : undefined
      }
      onPick={pick}
      byHand={
        <TextField
          path="capture.path"
          label="Enter the device by hand"
          placeholder="/dev/video0"
          value={value.path}
          onChange={(typed) => onChange({ ...value, path: typed })}
          check={(typed) =>
            typed.trim() ? null : 'Choose a device, or enter one by hand.'
          }
          help="Its name, such as /dev/video0."
        />
      }
      then={
        // Nothing is drawn for a device that delivers nothing.
        asked ? (
          <PictureChoice
            value={value}
            defaults={deviceDefaults}
            onChange={onChange}
            modes={modes}
            source="This device"
            unreachable="Plug in the device to choose its size and rate."
          />
        ) : null
      }
    />
  );
}
