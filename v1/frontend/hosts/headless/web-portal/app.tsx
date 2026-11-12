import type { WledSettings } from '@prysm/contracts/host-desktop';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import { WledOutputSettings } from '@prysm/output-wled-ui';
import {
  HeadlessCaptureSettings,
  type HeadlessCaptureConfig,
} from './capture-settings';

export function HeadlessWebPortal({
  application,
}: {
  application: ApplicationRuntime<HeadlessCaptureConfig, WledSettings>;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captureSettings={HeadlessCaptureSettings}
        outputSettings={WledOutputSettings}
      />
    </ApplicationProvider>
  );
}
