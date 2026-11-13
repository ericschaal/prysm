import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  describePicture,
  pictureSize,
  type AdapterUi,
} from '@prysm/web-portal';
import { DeviceEditor } from './editor';
import { deviceDefaults } from './settings';

export { deviceDefaults, deviceLimits } from './settings';

export const deviceCapture: AdapterUi<DeviceSettings> = {
  label: 'Video capture device',
  create: () => ({ kind: 'device', path: '' }),
  Editor: DeviceEditor,
  normalize: (value) => ({ ...value, path: value.path.trim() }),
  summary: ({ path, width, height, fps }) =>
    `${path}, ${describePicture(width ?? deviceDefaults.width, height ?? deviceDefaults.height, fps ?? deviceDefaults.fps)}`,
  // A device on this computer: nothing travels over a network.
  transport: () => 'local',
  facts: ({ width, height, fps }) => [
    {
      label: 'Picture',
      value: pictureSize(
        width ?? deviceDefaults.width,
        height ?? deviceDefaults.height,
      ),
    },
    { label: 'Rate', value: `${fps ?? deviceDefaults.fps} fps` },
  ],
};
