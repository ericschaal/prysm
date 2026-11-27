import { captureDefaults } from '@prysm/contracts/constants';
import type { FileSettings } from '@prysm/contracts/host-desktop';
import {
  PictureChoice,
  TextField,
  useCaptureModes,
  type EditorProps,
} from '@prysm/web-portal';

const fileQuery = { kind: 'file', path: '' } as const;

export function FileEditor({ value, onChange }: EditorProps<FileSettings>) {
  const modes = useCaptureModes(fileQuery);
  return (
    <>
      <TextField
        path="capture.path"
        label="Video file"
        required
        placeholder="/home/me/Videos/clip.mkv"
        value={value.path}
        onChange={(path) => onChange({ ...value, path })}
        help="Where the file is on this computer. It plays once, through FFmpeg 9 or newer."
      />
      <PictureChoice
        value={value}
        defaults={captureDefaults.file}
        onChange={onChange}
        modes={modes}
        source="A file"
        unreachable="Prysm can’t say what a file delivers just now."
        help="A slower file keeps its own rate."
      />
    </>
  );
}
