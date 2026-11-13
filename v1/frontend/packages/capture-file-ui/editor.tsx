import type { FileSettings } from '@prysm/contracts/host-desktop';
import { NumberField, TextField, type EditorProps } from '@prysm/web-portal';
import { fileDefaults, fileLimits } from './settings';

export function FileEditor({ value, onChange }: EditorProps<FileSettings>) {
  return (
    <>
      <p className="help">
        Plays a video file through FFmpeg 9 or newer, which must be installed on
        this machine. The Docker image and Debian package do not include FFmpeg,
        so this source does not work there.
      </p>
      <TextField
        path="capture.path"
        label="Video file"
        required
        placeholder="/home/me/Videos/clip.mkv"
        value={value.path}
        onChange={(path) => onChange({ ...value, path })}
        help="A path Prysm can read; any file FFmpeg 9 or later can decode. It plays once."
      />
      <div className="grid three">
        <NumberField
          path="capture.width"
          label="Width"
          unit="pixels"
          value={value.width ?? fileDefaults.width}
          onChange={(width) => onChange({ ...value, width })}
          {...fileLimits.width}
        />
        <NumberField
          path="capture.height"
          label="Height"
          unit="pixels"
          value={value.height ?? fileDefaults.height}
          onChange={(height) => onChange({ ...value, height })}
          {...fileLimits.height}
        />
        <NumberField
          path="capture.fps"
          label="Maximum frame rate"
          unit="fps"
          value={value.fps ?? fileDefaults.fps}
          onChange={(fps) => onChange({ ...value, fps })}
          {...fileLimits.fps}
          help="A slower file keeps its own rate."
        />
      </div>
    </>
  );
}
