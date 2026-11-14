import type {
  OutputSettings,
  CaptureSettings,
} from '@prysm/contracts/host-desktop';
import {
  httpApplicationClient,
  probePortal,
} from '@prysm/application-client/http';
import { lunaTvService } from '@prysm/webos-platform';
import type { PortalOptions } from '@prysm/tv-ui';
import { createRoot } from 'react-dom/client';
import { WebosSlimTvApp } from './app';

// The supervisor over Luna; the computer's Prysm (`preferences.instance`) over
// HTTP, which must allow this app's origin (`file://` sends `Origin: null`).
const portal: PortalOptions = {
  client: (baseUrl) =>
    httpApplicationClient<CaptureSettings, OutputSettings>({ baseUrl }),
  probe: (baseUrl) => probePortal(baseUrl),
};
const root = document.getElementById('root');
if (!root) throw new Error('Missing TV root.');
createRoot(root).render(
  <WebosSlimTvApp
    service={lunaTvService()}
    portal={portal}
    onExit={() => window.close()}
  />,
);
