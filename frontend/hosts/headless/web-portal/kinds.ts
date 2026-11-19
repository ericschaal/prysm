import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-desktop';
import type { AdapterUis } from '@prysm/web-portal';
import { deviceCapture } from '@prysm/capture-device-ui';
import { remoteCapture } from '@prysm/capture-remote-ui';
import { fileCapture } from '@prysm/capture-file-ui';
import { wledOutput } from '@prysm/output-wled-ui';
import { nullOutput } from '@prysm/output-null-ui';

/**
 * The kinds `prysm-headless` and `prysm-tui` register, in menu order. The
 * types are exhaustive over the generated unions, so a kind added to the host
 * (and the contract) fails to compile here until it has a panel.
 */
export const headlessCaptures: AdapterUis<CaptureSettings> = {
  device: deviceCapture,
  remote: remoteCapture,
  file: fileCapture,
};
export const headlessOutputs: AdapterUis<OutputSettings> = {
  wled: wledOutput,
  null: nullOutput,
};
