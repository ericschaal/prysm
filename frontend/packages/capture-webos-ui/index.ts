import { captureDefaults } from '@prysm/contracts/constants';
import type { WebosSettings } from '@prysm/contracts/host-webos';
import type { AdapterUi } from '@prysm/web-portal';
import { WebosEditor } from './editor';
import { describeTvPicture, tvPictureFacts } from './settings';

export { TvPictureFields } from './editor';
export * from './settings';

const defaults = captureDefaults.webos;

export const webosCapture: AdapterUi<WebosSettings> = {
  label: 'This TV',
  create: () => ({ kind: 'webos', ...defaults }),
  Editor: WebosEditor,
  summary: (value) => describeTvPicture(value, defaults),
  facts: (value) => tvPictureFacts(value, defaults),
  // The TV's own picture, captured on the TV.
  transport: () => 'built_in',
};
