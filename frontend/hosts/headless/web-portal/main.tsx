import {
  httpApplicationClient,
  httpDiscoveryClient,
  httpColorsClient,
  httpPreviewClient,
} from '@prysm/application-client/http';
import { createApplicationRuntime } from '@prysm/application-state';
import { scriptReplaced } from '@prysm/web-portal';
import { createRoot } from 'react-dom/client';
import {
  HeadlessWebPortal,
  type HeadlessCapture,
  type HeadlessOutput,
} from './app';

// The portal serves this page, so the client uses the page's own origin.
const application = createApplicationRuntime(
  httpApplicationClient<HeadlessCapture, HeadlessOutput>(),
);
const root = document.getElementById('root');
if (!root) throw new Error('Missing frontend root.');
createRoot(root).render(
  <HeadlessWebPortal
    application={application}
    preview={httpPreviewClient()}
    colors={httpColorsClient()}
    discovery={httpDiscoveryClient()}
    replaced={scriptReplaced(import.meta.url)}
  />,
);
