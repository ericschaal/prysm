import { ApplicationProvider } from '@prysm/application-react/provider';
import { createApplicationRuntime } from '@prysm/application-state';
import { useMemo } from 'react';
import type { ApplicationClient } from '@prysm/application-client';
import { TvUi, type TvUiProps } from '@prysm/tv-ui';

/** The slim TV forwards frames; lighting runs on the server it controls. */
export function WebosSlimApp<C, O>({
  remoteApplication,
  ...tv
}: Omit<TvUiProps<C, O>, 'application' | 'portalUrl'> & {
  remoteApplication: ApplicationClient<C, O>;
}) {
  const application = useMemo(
    () => createApplicationRuntime(remoteApplication),
    [remoteApplication],
  );
  return (
    <ApplicationProvider application={application}>
      <TvUi application={application} {...tv} />
    </ApplicationProvider>
  );
}
