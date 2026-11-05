import { createRoot } from 'react-dom/client';
import type {
  TvCaptureSettings as Capture,
  WledSettings as Output,
} from '@prysm/contracts/host-webos';
import { fakeApplicationPort } from '../../../testkit/application';
import { fakeTvHost, fakePatternPresenter } from '../../../testkit/tv';
import { WebosApp } from './app';

const application = fakeApplicationPort<Capture, Output>(
  {
    revision: '0',
    capture: { resolution: [320, 180], rate: 30 },
    output: { address: '192.0.2.1', leds: [10, 10, 10, 10] },
  },
  '1',
);
const platform = fakeTvHost(false);
const patterns = fakePatternPresenter();
const root = document.getElementById('root');
if (!root) throw new Error('Missing TV root.');
createRoot(root).render(
  <WebosApp
    localApplication={application.client}
    platform={platform.client}
    presenter={patterns.presenter}
    portalUrl="http://localhost:8787"
    onExit={() => {
      document.getElementById('demo-notice')!.textContent =
        'Fixture exit requested; lighting is unchanged.';
    }}
    onCleanupFailure={(error) => {
      console.error('Presentation cleanup unconfirmed', error);
    }}
  />,
);
