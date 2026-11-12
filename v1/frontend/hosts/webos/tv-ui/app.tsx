import type { WledSettings } from '@prysm/contracts/host-desktop';
import { ApplicationProvider } from '@prysm/application-react/provider';
import { createApplicationRuntime } from '@prysm/application-state';
import { useMemo } from 'react';
import type { ApplicationClient } from '@prysm/application-client';
import { TvUi, type TvUiProps } from '@prysm/tv-ui';
import type { WebosCaptureConfig } from '@prysm/capture-webos-ui';

export function WebosApp({
  localApplication,
  ...tv
}: Omit<TvUiProps<WebosCaptureConfig, WledSettings>, 'application'> & {
  localApplication: ApplicationClient<WebosCaptureConfig, WledSettings>;
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
