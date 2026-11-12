import type { WledSettings } from '@prysm/contracts/host-desktop';
import { httpApplicationClient } from '@prysm/application-client/http';
import { createApplicationRuntime } from '@prysm/application-state';
import { createRoot } from 'react-dom/client';
import type { HeadlessCaptureConfig } from './capture-settings';
import { HeadlessWebPortal } from './app';

// The portal serves this page, so the client uses the page's own origin.
const application = createApplicationRuntime(
  httpApplicationClient<HeadlessCaptureConfig, WledSettings>(),
);
const root = document.getElementById('root');
if (!root) throw new Error('Missing frontend root.');
createRoot(root).render(<HeadlessWebPortal application={application} />);
