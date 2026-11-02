import type { Config, Status } from './service';

export function capturePresentation(online: boolean | null, status: Status | null, draft: Config, dirty: boolean) {
    const enabled = status?.enabled;
    const connected = online && enabled && status?.receiverConnected;
    let title = 'Connecting…';
    let description = 'Checking the capture service on this TV.';
    let label = 'Connecting…';
    if (online === false) {
        title = 'TV service unavailable';
        description = 'Set up the background service to connect this TV to Prysm.';
        label = 'Set up service';
    } else if (online && status) {
        if (status.stopping) {
            title = 'Stopping capture…';
            description = 'Waiting for the sender to stop safely.';
        } else if (enabled && status.power !== 'Active') {
            title = 'Waiting for TV';
            description = 'The sender will reconnect when the TV is active.';
        } else if (enabled && status.videoAvailable === false) {
            title = 'Waiting for video';
            description = 'The sender is enabled. Select an HDMI input or play a video on the TV.';
        } else if (enabled) {
            title = connected ? (status.capturing ? 'Capturing' : 'Ready to capture') : 'Waiting for Prysm';
            description = connected
                ? (status.capturing ? 'Your TV picture is being sent to Prysm.' : 'Start capture in Prysm on your computer.')
                : 'Open Prysm on your computer and enable webOS input.';
        } else {
            title = 'Sender disabled';
            description = 'Enable the sender to connect to Prysm.';
        }
        if (status.error) {
            title = 'Sender needs attention';
            description = 'Open Service options for diagnostics or repair.';
        }
        label = enabled ? 'Disable sender' : !draft.receiver ? 'Set receiver address' : dirty ? 'Save and enable' : 'Enable sender';
    }

    return { title, description, label,
        ready: !!(connected && status?.power === 'Active' && status.videoAvailable !== false),
        attention: online === false || !!status?.error,
    };
}
