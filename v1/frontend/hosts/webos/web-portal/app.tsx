import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-webos';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal, type AdapterUis } from '@prysm/web-portal';
import { webosCapture } from '@prysm/capture-webos-ui';
import { wledOutput } from '@prysm/output-wled-ui';

export type WebosCapture = CaptureSettings;
export type WebosOutput = OutputSettings;

/** The kinds the `prysm-webos` worker registers. */
export const webosCaptures: AdapterUis<WebosCapture> = { webos: webosCapture };
export const webosOutputs: AdapterUis<WebosOutput> = { wled: wledOutput };

export function WebosWebPortal({
  application,
}: {
  application: ApplicationRuntime<WebosCapture, WebosOutput>;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captures={webosCaptures}
        outputs={webosOutputs}
      />
    </ApplicationProvider>
  );
}
