import { captureDefaults } from '@prysm/contracts/constants';
import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import {
  describeSavedPicture,
  pictureFacts,
  type AdapterUi,
} from '@prysm/web-portal';
import { DeviceEditor } from './editor';

export { typedDeviceMs } from './editor';

const defaults = captureDefaults.device;

export const deviceCapture: AdapterUi<DeviceSettings> = {
  label: 'Capture card or camera',
  create: () => ({ kind: 'device', device: '', ...defaults }),
  optional: defaults,
  Editor: DeviceEditor,
  normalize: (value) => ({ ...value, device: value.device.trim() }),
  summary: (value) =>
    `${value.device}, ${describeSavedPicture(value, defaults)}`,
  transport: () => 'local',
  problems: {
    'capture.device': 'Choose a device, or enter its path.',
    'capture.width': 'This device can’t capture this size. Choose another.',
    'capture.height': 'This device can’t capture this size. Choose another.',
    'capture.fps': 'This device can’t capture at this rate. Choose another.',
  },
  facts: (value) => pictureFacts(value, defaults),
};
