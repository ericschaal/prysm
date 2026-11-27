import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/host-webos';
import { WebPortal } from '@prysm/web-portal';
import type { BrowserPortalProps } from '../../browser-portal';
import { webosCaptures, webosOutputs } from './kinds';

export function WebosWebPortal(props: BrowserPortalProps) {
  return (
    <WebPortal<CaptureSettings, OutputSettings>
      {...props}
      captures={webosCaptures}
      outputs={webosOutputs}
    />
  );
}
