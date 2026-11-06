import type { WledSettings as WledOutputConfig } from '@prysm/contracts/host-server';
import type { ServerCaptureSettings as ServerCaptureConfig } from '@prysm/contracts/host-server';
import { createRoot } from 'react-dom/client';
import { fakeApplicationPort } from '../../../testkit/application';
import { createApplicationRuntime } from '@prysm/application-state';
import { ServerWebPortal } from './app';

const fixture = fakeApplicationPort<ServerCaptureConfig, WledOutputConfig>({
  revision: '0',
  capture: {
    adapter: 'device',
    settings: {
      source: 'device',
      path: '/dev/video0',
      resolution: [320, 180],
      rate: 30,
    },
  },
  output: { address: '192.0.2.1', leds: [10, 10, 10, 10] },
});
const application = createApplicationRuntime(fixture.client);
const root = document.getElementById('root');
if (!root) throw new Error('Missing frontend root.');
createRoot(root).render(<ServerWebPortal application={application} />);
