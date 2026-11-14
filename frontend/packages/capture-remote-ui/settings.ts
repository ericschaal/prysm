import type {
  RemoteReceiverSettings,
  StreamEncoding,
} from '@prysm/contracts/host-desktop';

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
    label: 'Compressed by the TV’s video chip (recommended)',
    help: 'Easiest for the TV and light on your network.',
  },
  {
    value: 'h264_software',
    label: 'Compressed by the TV’s processor',
    help: 'More work for the TV, equally light on your network.',
  },
  {
    value: 'raw',
    label: 'Uncompressed',
    help: 'Easiest for the TV, but about ten times the network traffic.',
  },
];

/** H.264 needs at least 16 × 16 (the Rust check on `capture.encoding`). */
export function encodingProblem(value: RemoteReceiverSettings): string | null {
  const encoding = value.encoding ?? remoteDefaults.encoding;
  const width = value.width ?? remoteDefaults.width;
  const height = value.height ?? remoteDefaults.height;
  return encoding !== 'raw' && (width < 16 || height < 16)
    ? 'A compressed picture needs at least 16 × 16 pixels; choose Uncompressed for smaller ones.'
    : null;
}
