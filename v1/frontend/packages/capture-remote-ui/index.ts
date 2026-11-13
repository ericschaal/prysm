import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { describeTvPicture, tvPictureFacts } from '@prysm/capture-webos-ui';
import type { AdapterUi } from '@prysm/web-portal';
import { RemoteEditor } from './editor';
import { remoteDefaults } from './settings';

export * from './settings';

export const remoteCapture: AdapterUi<RemoteReceiverSettings> = {
  label: 'Capture-only TV (remote)',
  create: () => ({ kind: 'remote' }),
  Editor: RemoteEditor,
  summary: (value) =>
    `From the TV, ${describeTvPicture(value, remoteDefaults)}`,
  facts: (value) => tvPictureFacts(value, remoteDefaults),
  // How the picture travels: H.264 (either encoder) or raw frames.
  transport: (value) =>
    (value.encoding ?? remoteDefaults.encoding) === 'raw' ? 'raw' : 'h264',
};
