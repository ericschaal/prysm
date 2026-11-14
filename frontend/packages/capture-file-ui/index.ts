import type { FileSettings } from '@prysm/contracts/host-desktop';
import {
  describeSavedPicture,
  pictureFacts,
  type AdapterUi,
} from '@prysm/web-portal';
import { FileEditor } from './editor';
import { fileDefaults } from './settings';

export { fileDefaults, fileLimits, filePresets } from './settings';

export const fileCapture: AdapterUi<FileSettings> = {
  // For development: it needs FFmpeg, which the packaged builds leave out.
  label: 'Video file (development only)',
  developer: true,
  create: () => ({ kind: 'file', path: '' }),
  Editor: FileEditor,
  normalize: (value) => ({ ...value, path: value.path.trim() }),
  summary: (value) =>
    `${value.path}, ${describeSavedPicture(value, fileDefaults)}`,
  transport: () => 'file',
  facts: (value) => pictureFacts(value, fileDefaults),
};
