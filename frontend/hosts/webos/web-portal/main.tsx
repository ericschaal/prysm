import {
  httpApplicationClient,
  httpDiscoveryClient,
  httpColorsClient,
  httpPreviewClient,
} from '@prysm/application-client/http';
import { createApplicationRuntime } from '@prysm/application-state';
import { scriptReplaced } from '@prysm/web-portal';
import { createRoot } from 'react-dom/client';
import { WebosWebPortal, type WebosCapture, type WebosOutput } from './app';

// The portal serves this page, so the client uses the page's own origin.
const application = createApplicationRuntime(
  httpApplicationClient<WebosCapture, WebosOutput>(),
);
const root = document.getElementById('root');
if (!root) throw new Error('Missing frontend root.');
createRoot(root).render(
  <WebosWebPortal
    application={application}
    preview={httpPreviewClient()}
    colors={httpColorsClient()}
    discovery={httpDiscoveryClient()}
    replaced={scriptReplaced(import.meta.url)}
  />,
);
