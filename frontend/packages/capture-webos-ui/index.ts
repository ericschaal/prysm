import type { WebosSettings } from '@prysm/contracts/host-webos';
import type { AdapterUi } from '@prysm/web-portal';
import { WebosEditor } from './editor';
import { describeTvPicture, tvPictureFacts, webosDefaults } from './settings';

export { TvPictureFields } from './editor';
export * from './settings';

export const webosCapture: AdapterUi<WebosSettings> = {
  label: 'This TV',
  create: () => ({ kind: 'webos' }),
  Editor: WebosEditor,
  summary: (value) => describeTvPicture(value, webosDefaults),
  facts: (value) => tvPictureFacts(value, webosDefaults),
  // The TV's own picture, captured on the TV.
  transport: () => 'built_in',
};
