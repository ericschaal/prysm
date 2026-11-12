import type { FileSettings } from '@prysm/contracts/host-desktop';
import {
  NumberField,
  TextField,
  type AdapterUi,
  type EditorProps,
} from '@prysm/web-portal';

/**
 * Limits from the generated `FileSettings` schema. `fps` has no schema
 * maximum; the Rust `u32` bounds it.
 */
export const fileLimits = {
  width: { min: 1, max: 3840, step: 1 },
  height: { min: 1, max: 2160, step: 1 },
  fps: { min: 1, max: 4294967295, step: 1 },
} as const;
/** What Rust uses for an omitted field. */
export const fileDefaults = { width: 640, height: 360, fps: 30 } as const;

function FileEditor({ value, onChange }: EditorProps<FileSettings>) {
  return (
    <>
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

export const fileCapture: AdapterUi<FileSettings> = {
  label: 'Video file',
  create: () => ({ kind: 'file', path: '' }),
  Editor: FileEditor,
  normalize: (value) => ({ ...value, path: value.path.trim() }),
};
