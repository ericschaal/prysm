import type { StreamEncoding } from '@prysm/contracts/host-desktop';

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
