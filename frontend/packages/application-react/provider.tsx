import { useEffect, type ReactNode } from 'react';
import { Provider } from 'react-redux';
import type { ApplicationRuntime } from '@prysm/application-state';

export function ApplicationProvider<C, O>({
  application,
  children,
}: {
  application: ApplicationRuntime<C, O>;
  children: ReactNode;
}) {
  useEffect(() => application.connect(), [application]);
  return <Provider store={application.store}>{children}</Provider>;
}
