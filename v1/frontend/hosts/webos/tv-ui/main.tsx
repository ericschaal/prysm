import type {
  WebosSettings,
  WledSettings,
} from '@prysm/contracts/host-webos';
import { createRoot } from 'react-dom/client';
import { fakeApplicationPort } from '../../../testkit/application';
import { fakeTvHost } from '../../../testkit/tv';
import { WebosApp } from './app';

// The Luna bridge is not implemented; the TV entry point runs on a fixture.
const application = fakeApplicationPort<WebosSettings, WledSettings>({
  capture: { kind: 'webos', fps: 30 },
  output: { kind: 'wled', address: '192.0.2.1', leds: { top: 10, left: 10 } },
});
const platform = fakeTvHost(false);
const root = document.getElementById('root');
if (!root) throw new Error('Missing TV root.');
createRoot(root).render(
  <WebosApp
    localApplication={application.client}
    platform={platform.client}
    portalUrl="http://localhost:8787"
    onExit={() => {
      document.getElementById('demo-notice')!.textContent =
        'Fixture exit requested; lighting is unchanged.';
    }}
  />,
);
