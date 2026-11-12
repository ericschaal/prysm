import type {
  WebosSettings,
  WledSettings,
} from '@prysm/contracts/host-webos';
import { ApplicationProvider } from '@prysm/application-react/provider';
import { createApplicationRuntime } from '@prysm/application-state';
import { useMemo } from 'react';
import type { ApplicationClient } from '@prysm/application-client';
import { TvUi, type TvUiProps } from '@prysm/tv-ui';

export function WebosApp({
  localApplication,
  ...tv
}: Omit<TvUiProps<WebosSettings, WledSettings>, 'application'> & {
  localApplication: ApplicationClient<WebosSettings, WledSettings>;
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
