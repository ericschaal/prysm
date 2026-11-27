import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-desktop';
import { WebPortal } from '@prysm/web-portal';
import type { BrowserPortalProps } from '../../browser-portal';
import { headlessCaptures, headlessOutputs } from './kinds';

export function HeadlessWebPortal(props: BrowserPortalProps) {
  return (
    <WebPortal<CaptureSettings, OutputSettings>
      {...props}
      captures={headlessCaptures}
      outputs={headlessOutputs}
    />
  );
}
