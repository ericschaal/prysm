import {
  httpApplicationClient,
  probePortal,
} from '@prysm/application-client/http';
import { lunaTvService } from '@prysm/webos-platform';
import type { PortalOptions, TvAppProps } from '@prysm/tv-ui';
import type { ComponentType } from 'react';
import { createRoot } from 'react-dom/client';

const portal: PortalOptions = {
  client: (baseUrl) => httpApplicationClient({ baseUrl }),
  probe: probePortal,
};

/** Mount the installed TV flavor with its Luna service and HTTP portal. */
export function mountTvApp(App: ComponentType<Omit<TvAppProps, 'flavor'>>) {
  const root = document.getElementById('root');
  if (!root) throw new Error('Missing TV root.');
  createRoot(root).render(
    <App
      service={lunaTvService()}
      portal={portal}
      onExit={() => window.close()}
    />,
  );
}
