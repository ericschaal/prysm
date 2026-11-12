import { httpApplicationClient } from '@prysm/application-client/http';
import { createApplicationRuntime } from '@prysm/application-state';
import { createRoot } from 'react-dom/client';
import { WebosWebPortal, type WebosCapture, type WebosOutput } from './app';

// The portal serves this page, so the client uses the page's own origin.
const application = createApplicationRuntime(
  httpApplicationClient<WebosCapture, WebosOutput>(),
);
const root = document.getElementById('root');
if (!root) throw new Error('Missing frontend root.');
createRoot(root).render(<WebosWebPortal application={application} />);
