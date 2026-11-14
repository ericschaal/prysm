import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-webos';
import type { AdapterUis } from '@prysm/web-portal';
import { webosCapture } from '@prysm/capture-webos-ui';
import { wledOutput } from '@prysm/output-wled-ui';

/** The kinds the `prysm-webos` worker registers. */
export const webosCaptures: AdapterUis<CaptureSettings> = {
  webos: webosCapture,
};
export const webosOutputs: AdapterUis<OutputSettings> = { wled: wledOutput };
