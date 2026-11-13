import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-desktop';
import type { PreviewClient } from '@prysm/application-client';
import { ApplicationProvider } from '@prysm/application-react/provider';
import type { ApplicationRuntime } from '@prysm/application-state';
import { WebPortal } from '@prysm/web-portal';
import { headlessCaptures, headlessOutputs } from './kinds';

export type HeadlessCapture = CaptureSettings;
export type HeadlessOutput = OutputSettings;

export function HeadlessWebPortal({
  application,
  preview,
}: {
  application: ApplicationRuntime<HeadlessCapture, HeadlessOutput>;
  preview?: PreviewClient;
}) {
  return (
    <ApplicationProvider application={application}>
      <WebPortal
        application={application}
        captures={headlessCaptures}
        preview={preview}
        outputs={headlessOutputs}
      />
    </ApplicationProvider>
  );
}
