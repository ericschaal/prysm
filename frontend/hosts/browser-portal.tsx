import {
  embeddedState,
  httpApplicationClient,
  httpPortalClient,
} from '@prysm/application-client/http';
import { createApplicationRuntime } from '@prysm/application-state';
import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/application';
import { scriptReplaced, type WebPortalProps } from '@prysm/web-portal';
import type { ComponentType } from 'react';
import { createRoot } from 'react-dom/client';

export type BrowserPortalProps = Omit<
  WebPortalProps<CaptureSettings, OutputSettings>,
  'captures' | 'outputs'
>;

/** Both browser hosts serve the API and embed the first snapshot in the page. */
export function mountBrowserPortal(
  Portal: ComponentType<BrowserPortalProps>,
  scriptUrl: string,
) {
  const root = document.getElementById('root');
  if (!root) throw new Error('Missing frontend root.');
  const application = createApplicationRuntime(
    httpApplicationClient(),
    embeddedState(),
  );
  const client = httpPortalClient();
  createRoot(root).render(
    <Portal
      application={application}
      preview={client}
      colors={client}
      discovery={client}
      captureModes={client}
      replaced={scriptReplaced(scriptUrl)}
    />,
  );
}
