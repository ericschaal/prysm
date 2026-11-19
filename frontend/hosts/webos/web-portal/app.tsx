import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-webos';
import type {
  CaptureModesClient,
  ColorsClient,
  DiscoveryClient,
  PreviewClient,
} from '@prysm/application-client';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import { webosCaptures, webosOutputs } from './kinds';

export type WebosCapture = CaptureSettings;
export type WebosOutput = OutputSettings;

export function WebosWebPortal({
  application,
  preview,
  colors,
  discovery,
  captureModes,
  replaced,
}: {
  application: ApplicationRuntime<WebosCapture, WebosOutput>;
  preview?: PreviewClient;
  colors?: ColorsClient;
  discovery?: DiscoveryClient;
  captureModes: CaptureModesClient;
  replaced?: () => Promise<boolean>;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captures={webosCaptures}
        preview={preview}
        colors={colors}
        discovery={discovery}
        captureModes={captureModes}
        replaced={replaced}
        outputs={webosOutputs}
      />
    </ApplicationProvider>
  );
}
