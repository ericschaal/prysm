import { ApplicationProvider } from '@prysm/application-react/provider';
import { createApplicationRuntime } from '@prysm/application-state';
import { useMemo } from 'react';
import { type ApplicationClient } from '@prysm/application-client';
import { TvUi, type TvUiProps } from '@prysm/tv-ui';
import type {
  TvCaptureSettings as Capture,
  WledSettings as Output,
} from '@prysm/contracts/host-webos';

export function WebosApp({
  localApplication,
  ...tv
}: Omit<TvUiProps<Capture, Output>, 'application'> & {
  localApplication: ApplicationClient<Capture, Output>;
  portalUrl: string;
}) {
  const application = useMemo(
    () => createApplicationRuntime(localApplication),
    [localApplication],
  );
  return (
    <ApplicationProvider application={application}>
      <TvUi application={application} {...tv} />
    </ApplicationProvider>
  );
}
