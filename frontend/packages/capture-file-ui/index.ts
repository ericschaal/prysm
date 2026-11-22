import { captureDefaults } from '@prysm/contracts/constants';
import type { FileSettings } from '@prysm/contracts/host-desktop';
import {
  describeSavedPicture,
  pictureFacts,
  type AdapterUi,
} from '@prysm/web-portal';
import { FileEditor } from './editor';

const defaults = captureDefaults.file;

export const fileCapture: AdapterUi<FileSettings> = {
  // For development: it needs FFmpeg, which the packaged builds leave out.
  label: 'Video file (development only)',
  developer: true,
  create: () => ({ kind: 'file', path: '', ...defaults }),
  Editor: FileEditor,
  normalize: (value) => ({ ...value, path: value.path.trim() }),
  summary: (value) => `${value.path}, ${describeSavedPicture(value, defaults)}`,
  transport: () => 'file',
  facts: (value) => pictureFacts(value, defaults),
};
