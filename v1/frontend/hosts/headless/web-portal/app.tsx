import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-desktop';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal, type AdapterUis } from '@prysm/web-portal';
import { deviceCapture } from '@prysm/capture-device-ui';
import { remoteCapture } from '@prysm/capture-remote-ui';
import { fileCapture } from '@prysm/capture-file-ui';
import { wledOutput } from '@prysm/output-wled-ui';
import { nullOutput } from '@prysm/output-null-ui';

export type HeadlessCapture = CaptureSettings;
export type HeadlessOutput = OutputSettings;

/**
 * The kinds `prysm-headless` and `prysm-tui` register, in menu order. The
 * types are exhaustive over the generated unions, so a kind added to the host
 * (and the contract) fails to compile here until it has a panel.
 */
export const headlessCaptures: AdapterUis<HeadlessCapture> = {
  device: deviceCapture,
  remote: remoteCapture,
  file: fileCapture,
};
export const headlessOutputs: AdapterUis<HeadlessOutput> = {
  wled: wledOutput,
  null: nullOutput,
};

export function HeadlessWebPortal({
  application,
}: {
  application: ApplicationRuntime<HeadlessCapture, HeadlessOutput>;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captures={headlessCaptures}
        outputs={headlessOutputs}
      />
    </ApplicationProvider>
  );
}
