import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { describeTvPicture } from '@prysm/capture-webos-ui';
import type { AdapterUi } from '@prysm/web-portal';
import { RemoteEditor } from './editor';
import { remoteDefaults } from './settings';

export * from './settings';

export const remoteCapture: AdapterUi<RemoteReceiverSettings> = {
  label: 'Capture-only TV (remote)',
  create: () => ({ kind: 'remote', listen: '' }),
  Editor: RemoteEditor,
  normalize: (value) => ({ ...value, listen: value.listen.trim() }),
  summary: (value) =>
    `Listens on ${value.listen}, ${describeTvPicture(value, remoteDefaults)}`,
};
