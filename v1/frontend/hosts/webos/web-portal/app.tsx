import type {
  WebosSettings,
  WledSettings,
} from '@prysm/contracts/host-webos';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import { WebosCaptureSettings } from '@prysm/capture-webos-ui';
import { WledOutputSettings } from '@prysm/output-wled-ui';

export function WebosWebPortal({
  application,
}: {
  application: ApplicationRuntime<WebosSettings, WledSettings>;
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
