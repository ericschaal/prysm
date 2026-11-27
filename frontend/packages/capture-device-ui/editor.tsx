import { useEffect, useState } from 'react';
import { captureDefaults } from '@prysm/contracts/constants';
import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  DeviceRows,
  PictureChoice,
  presetRates,
  useCaptureModes,
  type EditorProps,
  type ModesState,
} from '@prysm/web-portal';

/** How long typing pauses before the typed device is asked about. */
export const typedDeviceMs = 400;

/**
 * Why the device leaves nothing to choose, if it does: it lists nothing
 * Prysm captures, or nothing at a preset's rate and size.
 */
function problemOf(modes: ModesState) {
  if (modes.state !== 'known' || modes.offered.length > 0) return undefined;
  return modes.modes.length === 0
    ? 'This device offers no picture Prysm can use.'
    : `This device doesn’t offer ${presetRates} at a size Prysm can use.`;
}

const typedDevice = {
  choice: 'Another device',
  label: 'Device path',
  placeholder: '/dev/video0',
  empty: 'Choose a device, or enter its path.',
};

/**
 * The device as rows from discovery, the last one typed, then its sizes
 * and rates once it is known which device: asked of the device itself.
 */
export function DeviceEditor({ value, onChange }: EditorProps<DeviceSettings>) {
  const device = value.device.trim();
  // A typed device is asked about once typing pauses; a picked one at once.
  const [asked, setAsked] = useState(device);
  useEffect(() => {
    const timer = setTimeout(() => setAsked(device), typedDeviceMs);
    return () => clearTimeout(timer);
  }, [device]);
  const modes = useCaptureModes(
    asked ? { kind: 'device', device: asked } : null,
  );
  const pick = (next: string) => {
    setAsked(next);
    onChange({ ...value, device: next });
  };
  return (
    <>
      <DeviceRows
        section="capture"
        kind="device"
        field="device"
        legend="Device"
        things="devices"
        nothing="No devices found. Plug one in, or enter its path."
        value={device}
        text={value.device}
        typed={typedDevice}
        missing={modes.state === 'unknown' ? 'Not plugged in' : undefined}
        problem={asked ? problemOf(modes) : undefined}
        onPick={pick}
        onType={(typed) => onChange({ ...value, device: typed })}
        onLook={'again' in modes ? modes.again : undefined}
      />
      {asked ? (
        <PictureChoice
          value={value}
          defaults={captureDefaults.device}
          onChange={onChange}
          modes={modes}
          source="This device"
          unreachable="Plug in the device to choose its size and rate."
        />
      ) : null}
    </>
  );
}
