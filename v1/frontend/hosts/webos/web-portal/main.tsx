import type { WledSettings as WledOutputConfig } from '@prysm/contracts/host-webos';
import type { TvCaptureSettings as WebosCaptureConfig } from '@prysm/contracts/host-webos';
import { createRoot } from 'react-dom/client';
import { fakeApplicationPort } from '../../../testkit/application';
import { createApplicationRuntime } from '@prysm/application-state';
import { WebosWebPortal } from './app';

const fixture = fakeApplicationPort<WebosCaptureConfig, WledOutputConfig>({
  revision: '0',
  capture: { resolution: [320, 180], rate: 30 },
  output: { address: '192.0.2.1', leds: [10, 10, 10, 10] },
});
const application = createApplicationRuntime(fixture.client);
const root = document.getElementById('root');
if (!root) throw new Error('Missing frontend root.');
createRoot(root).render(<WebosWebPortal application={application} />);
