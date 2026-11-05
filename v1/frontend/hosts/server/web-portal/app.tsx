import { ApplicationProvider } from '@prysm/application-react/provider';
import type { WledSettings as WledOutputConfig } from '@prysm/contracts/host-server';
import type { ServerCaptureSettings as ServerCaptureConfig } from '@prysm/contracts/host-server';
import { WebPortal } from '@prysm/web-portal';
import type { ApplicationRuntime } from '@prysm/application-state';
import { ServerCaptureSettings } from './capture-settings';
import { WledOutputSettings } from '@prysm/output-wled-ui';

export function ServerWebPortal({
  application,
}: {
  application: ApplicationRuntime<ServerCaptureConfig, WledOutputConfig>;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captureSettings={ServerCaptureSettings}
        outputSettings={WledOutputSettings}
      />
    </ApplicationProvider>
  );
}
