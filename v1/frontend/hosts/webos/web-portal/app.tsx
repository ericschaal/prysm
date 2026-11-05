import { ApplicationProvider } from '@prysm/application-react/provider';
import type { WledSettings as WledOutputConfig } from '@prysm/contracts/host-webos';
import type { TvCaptureSettings as WebosCaptureConfig } from '@prysm/contracts/host-webos';
import { WebPortal } from '@prysm/web-portal';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebosCaptureSettings } from '@prysm/capture-webos-ui';
import { WledOutputSettings } from '@prysm/output-wled-ui';

export function WebosWebPortal({
  application,
}: {
  application: ApplicationRuntime<WebosCaptureConfig, WledOutputConfig>;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captureSettings={WebosCaptureSettings}
        outputSettings={WledOutputSettings}
      />
    </ApplicationProvider>
  );
}
