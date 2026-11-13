import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-webos';
import type { DiscoveryClient, PreviewClient } from '@prysm/application-client';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import { webosCaptures, webosOutputs } from './kinds';

export type WebosCapture = CaptureSettings;
export type WebosOutput = OutputSettings;

export function WebosWebPortal({
  application,
  preview,
  discovery,
}: {
  application: ApplicationRuntime<WebosCapture, WebosOutput>;
  preview?: PreviewClient;
  discovery?: DiscoveryClient;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captures={webosCaptures}
        preview={preview}
        discovery={discovery}
        outputs={webosOutputs}
      />
    </ApplicationProvider>
  );
}
