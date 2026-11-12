import type { WledSettings } from '@prysm/contracts/host-desktop';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import {
  WebosCaptureSettings,
  type WebosCaptureConfig,
} from '@prysm/capture-webos-ui';
import { WledOutputSettings } from '@prysm/output-wled-ui';

export function WebosWebPortal({
  application,
}: {
  application: ApplicationRuntime<WebosCaptureConfig, WledSettings>;
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
