import type { Config, Status } from './service';
import { embeddedBuild, supportsEmbedded } from './processingLocation';

export function capturePresentation(
  online: boolean | null,
  status: Status | null,
  draft: Config,
  dirty: boolean,
) {
  const enabled = status?.enabled;
  const embedded =
    embeddedBuild &&
    supportsEmbedded(status) &&
    status?.config.location === 'embedded';
  const connected = online && enabled && status?.receiverConnected;
  let title = 'Connecting…';
  let description = 'Checking the capture service on this TV.';
  let label = 'Connecting…';
  if (online === false) {
    title = 'TV service unavailable';
    description = 'Set up the background service to connect this TV to Prysm.';
    label = 'Set up service';
  } else if (online && status) {
    if (
      !embedded &&
      enabled &&
      status.power === 'Active' &&
      status.videoAvailable === false
    ) {
      title = 'Waiting for video';
      description = 'Select an HDMI input or play a video on the TV.';
    } else if (status.stopping) {
      title = 'Stopping capture…';
      description = 'Finishing the current capture session.';
    } else if (enabled && status.power !== 'Active') {
      title = 'Waiting for TV';
      description =
        embeddedBuild && embedded
          ? 'Local capture will resume when the TV wakes up.'
          : 'Prysm will reconnect when the TV wakes up.';
    } else if (embeddedBuild && embedded) {
      if (!enabled) {
        title = 'TV capture paused';
        description = 'The portal stays available while capture is paused.';
      } else if (status.capturing) {
        title = 'Capturing';
        description =
          'This TV is processing the picture and sending colors to WLED.';
      } else {
        title = status.portalUrl ? 'Prysm is ready' : 'Starting Prysm';
        description = status.portalUrl
          ? 'Use the web portal to set up your lights or start screen sync.'
          : 'Waiting for the web portal to open.';
      }
    } else if (enabled && status.receiverBusy) {
      title = 'Receiver in use';
      description =
        'Another TV is capturing. This TV will retry automatically.';
    } else if (enabled && !connected) {
      title = 'Waiting for Prysm';
      description =
        'This TV will reconnect to your saved instance automatically.';
    } else if (enabled && status.capturing) {
      title = 'Capturing';
      description = 'Your TV picture is being sent to Prysm.';
    } else if (enabled) {
      title = 'Ready to capture';
      description = 'Start capture in Prysm on your computer.';
    } else {
      title = 'Disabled';
      description = 'Enable capture to connect this TV to Prysm.';
    }
    if (status.error) {
      title = 'Capture needs attention';
      description = 'Open Service options for diagnostics or repair.';
    }
    if (enabled) label = embedded ? 'Pause TV capture' : 'Disable';
    else if (
      embeddedBuild &&
      supportsEmbedded(status) &&
      draft.location === 'embedded'
    ) {
      label = dirty && !embedded ? 'Use this TV' : 'Allow TV capture';
    } else if (draft.location !== 'embedded' && !draft.instanceUrl)
      label = 'Set receiver address';
    else if (dirty) label = 'Save and enable';
    else label = 'Enable';
  }

  return {
    title,
    description,
    label,
    ready: !!(
      (embedded ? status?.capturing : connected) &&
      status?.power === 'Active' &&
      status.videoAvailable !== false
    ),
    attention: online === false || !!status?.error,
  };
}
