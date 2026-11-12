import type {
  RemoteReceiverSettings,
  StreamEncoding,
} from '@prysm/contracts/host-desktop';
import { describeTvPicture, TvPictureFields } from '@prysm/capture-webos-ui';
import {
  SelectField,
  TextField,
  useCheck,
  type AdapterUi,
  type EditorProps,
} from '@prysm/web-portal';

/** The size hint of the stream form: a larger stream also costs network. */
export const largeStreamHint =
  '640\u00a0×\u00a0360 is enough for the lights. Larger sizes use more of the TV’s processor and more network without a visible difference.';

/** What Rust uses for an omitted field. */
export const remoteDefaults = {
  width: 320,
  height: 180,
  fps: 30,
  include_ui: true,
  encoding: 'h264_hardware',
} as const;

/** The stream encodings in the order offered, each with its trade-off. */
export const encodingChoices: readonly {
  value: StreamEncoding;
  label: string;
  help: string;
}[] = [
  {
    value: 'h264_hardware',
    label: 'Hardware H.264 (recommended)',
    help: 'Least work for the TV, little network (about 2 Mbit/s).',
  },
  {
    value: 'h264_software',
    label: 'Software H.264',
    help: 'More work for the TV’s processor, the same little network.',
  },
  {
    value: 'raw',
    label: 'Uncompressed',
    help: 'Least work for the TV, but about ten times the network.',
  },
];

/** H.264 needs at least 16 × 16 (the Rust check on `capture.encoding`). */
export function encodingProblem(value: RemoteReceiverSettings): string | null {
  const encoding = value.encoding ?? remoteDefaults.encoding;
  const width = value.width ?? remoteDefaults.width;
  const height = value.height ?? remoteDefaults.height;
  return encoding !== 'raw' && (width < 16 || height < 16)
    ? 'H.264 needs at least 16 × 16 pixels; choose Uncompressed for smaller pictures.'
    : null;
}

function RemoteEditor({ value, onChange }: EditorProps<RemoteReceiverSettings>) {
  const encoding = value.encoding ?? remoteDefaults.encoding;
  useCheck('capture.encoding', encodingProblem(value));
  return (
    <>
      <TextField
        path="capture.listen"
        label="Listen address"
        required
        placeholder="0.0.0.0:7380"
        value={value.listen}
        onChange={(listen) => onChange({ ...value, listen })}
        help="Address and fixed port this computer listens on for frames from a capture-only TV, such as 0.0.0.0:7380."
      />
      <TvPictureFields
        value={value}
        defaults={remoteDefaults}
        sizeHint={largeStreamHint}
        onChange={onChange}
      />
      <SelectField
        path="capture.encoding"
        label="Stream encoding"
        value={encoding}
        options={encodingChoices}
        onChange={(next) => onChange({ ...value, encoding: next })}
        help={encodingChoices.find((choice) => choice.value === encoding)?.help}
      />
      <p className="help">
        The TV sends this picture to this computer. {'320\u00a0×\u00a0180'} at
        30 fps is plenty for lighting. If the TV cannot send it, lighting stops and says
        why; change the encoding or size here and start screen sync again.
      </p>
    </>
  );
}

export const remoteCapture: AdapterUi<RemoteReceiverSettings> = {
  label: 'Capture-only TV (remote)',
  create: () => ({ kind: 'remote', listen: '' }),
  Editor: RemoteEditor,
  normalize: (value) => ({ ...value, listen: value.listen.trim() }),
  summary: (value) =>
    `Listens on ${value.listen}, ${describeTvPicture(value, remoteDefaults)}`,
};
