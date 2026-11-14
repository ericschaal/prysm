import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-desktop';
import type {
  ColorsClient,
  DiscoveryClient,
  PreviewClient,
} from '@prysm/application-client';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import { headlessCaptures, headlessOutputs } from './kinds';

export type HeadlessCapture = CaptureSettings;
export type HeadlessOutput = OutputSettings;

export function HeadlessWebPortal({
  application,
  preview,
  colors,
  discovery,
  replaced,
}: {
  application: ApplicationRuntime<HeadlessCapture, HeadlessOutput>;
  preview?: PreviewClient;
  colors?: ColorsClient;
  discovery?: DiscoveryClient;
  replaced?: () => Promise<boolean>;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captures={headlessCaptures}
        preview={preview}
        colors={colors}
        discovery={discovery}
        replaced={replaced}
        outputs={headlessOutputs}
      />
    </ApplicationProvider>
  );
}
