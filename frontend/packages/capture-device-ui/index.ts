import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  describeSavedPicture,
  pictureFacts,
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
  summary: (value) =>
    `${value.path}, ${describeSavedPicture(value, deviceDefaults)}`,
  // A device on this computer: nothing travels over a network.
  transport: () => 'local',
  problems: {
    'capture.path': 'Pick a device with Find devices, or type its name.',
    'capture.width': 'Use a size the device supports.',
    'capture.height': 'Use a size the device supports.',
    'capture.fps': 'Use a rate the device supports.',
  },
  facts: (value) => pictureFacts(value, deviceDefaults),
};
