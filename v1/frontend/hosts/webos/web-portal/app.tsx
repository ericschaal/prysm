import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-webos';
import type { PreviewClient } from '@prysm/application-client';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import { webosCaptures, webosOutputs } from './kinds';

export type WebosCapture = CaptureSettings;
export type WebosOutput = OutputSettings;

export function WebosWebPortal({
  application,
  preview,
}: {
  application: ApplicationRuntime<WebosCapture, WebosOutput>;
  preview?: PreviewClient;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captures={webosCaptures}
        preview={preview}
        outputs={webosOutputs}
      />
    </ApplicationProvider>
  );
}
