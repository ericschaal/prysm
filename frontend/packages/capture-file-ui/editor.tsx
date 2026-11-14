import type { FileSettings } from '@prysm/contracts/host-desktop';
import { PictureChoice, TextField, type EditorProps } from '@prysm/web-portal';
import { fileDefaults, fileLimits, filePresets } from './settings';

export function FileEditor({ value, onChange }: EditorProps<FileSettings>) {
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
        presets={filePresets}
        limits={fileLimits}
        rate={{
          label: 'Maximum frame rate',
          help: 'A slower file keeps its own rate.',
        }}
      />
    </>
  );
}
