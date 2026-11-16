import type { FileSettings } from '@prysm/contracts/host-desktop';
import {
  PictureChoice,
  TextField,
  useCaptureModes,
  type EditorProps,
} from '@prysm/web-portal';
import { fileDefaults } from './settings';

/** Every file gets the same answer: FFmpeg scales it. */
const fileQuery = { kind: 'file', path: '' } as const;

export function FileEditor({ value, onChange }: EditorProps<FileSettings>) {
  const modes = useCaptureModes(fileQuery);
  return (
    <>
      <p className="help">
        Plays a video file instead of a live picture, for testing. Needs FFmpeg
        9 or newer on this computer.
      </p>
      <TextField
        path="capture.path"
        label="Video file"
        required
        placeholder="/home/me/Videos/clip.mkv"
        value={value.path}
        onChange={(path) => onChange({ ...value, path })}
        help="Where the file is on this computer. It plays once."
      />
      <PictureChoice
        value={value}
        defaults={fileDefaults}
        onChange={onChange}
        modes={modes}
        source="A file"
        unreachable="Prysm can’t say what a file delivers just now."
      />
      <p className="help">A slower file keeps its own rate.</p>
    </>
  );
}
