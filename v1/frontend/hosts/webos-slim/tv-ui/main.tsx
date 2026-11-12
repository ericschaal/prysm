import type {
  RemoteReceiverSettings,
  WledSettings,
} from '@prysm/contracts/host-desktop';
import { createRoot } from 'react-dom/client';
import { fakeApplicationPort } from '../../../testkit/application';
import { fakeTvHost } from '../../../testkit/tv';
import { WebosSlimApp } from './app';

// The remote application client is not implemented; this runs on a fixture.
const application = fakeApplicationPort<RemoteReceiverSettings, WledSettings>(
  {
    capture: { kind: 'remote', listen: '0.0.0.0:9000' },
    output: { kind: 'wled', address: '192.0.2.1', leds: { top: 10, left: 10 } },
  },
);
const platform = fakeTvHost(true);
const root = document.getElementById('root');
if (!root) throw new Error('Missing TV root.');
createRoot(root).render(
  <WebosSlimApp
    remoteApplication={application.client}
    platform={platform.client}
    onExit={() => {
      document.getElementById('demo-notice')!.textContent =
        'Fixture exit requested; lighting is unchanged.';
    }}
  />,
);
