import type { WebosSettings, WledSettings } from '@prysm/contracts/host-webos';
import {
  httpApplicationClient,
  probePortal,
} from '@prysm/application-client/http';
import { lunaTvService } from '@prysm/webos-platform';
import type { PortalOptions } from '@prysm/tv-ui';
import { createRoot } from 'react-dom/client';
import { WebosTvApp } from './app';

// The supervisor over Luna; the TV's own portal over HTTP (it allows any origin).
const portal: PortalOptions = {
  client: (baseUrl) =>
    httpApplicationClient<WebosSettings, WledSettings>({ baseUrl }),
  probe: (baseUrl) => probePortal(baseUrl),
};
const root = document.getElementById('root');
if (!root) throw new Error('Missing TV root.');
createRoot(root).render(
  <WebosTvApp
    service={lunaTvService()}
    portal={portal}
    onExit={() => window.close()}
  />,
);
