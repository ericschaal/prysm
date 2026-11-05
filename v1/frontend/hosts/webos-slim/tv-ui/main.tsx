import { createRoot } from 'react-dom/client';
import type {
  ServerCaptureSettings as Capture,
  WledSettings as Output,
} from '@prysm/contracts/host-server';
import { fakeApplicationPort } from '../../../testkit/application';
import { fakeTvHost, fakePatternPresenter } from '../../../testkit/tv';
import { WebosSlimApp } from './app';

const application = fakeApplicationPort<Capture, Output>(
  {
    revision: '0',
    capture: {
      adapter: 'remote',
      settings: { peer: '192.0.2.10:9000' },
    },
    output: { address: '192.0.2.1', leds: [10, 10, 10, 10] },
  },
  '2',
);
const platform = fakeTvHost(true);
const patterns = fakePatternPresenter();
const root = document.getElementById('root');
if (!root) throw new Error('Missing TV root.');
createRoot(root).render(
  <WebosSlimApp
    remoteApplication={application.client}
    platform={platform.client}
    presenter={patterns.presenter}

    onExit={() => {
      document.getElementById('demo-notice')!.textContent =
        'Fixture exit requested; lighting is unchanged.';
    }}
    onCleanupFailure={(error) => {
      console.error('Presentation cleanup unconfirmed', error);
    }}
  />,
);
