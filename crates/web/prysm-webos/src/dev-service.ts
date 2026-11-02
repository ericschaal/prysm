import type { Config, Status, ServiceBridge } from './service';

// In-memory Luna simulation for the Vite dev server; removed from TV builds.
export function installDevService() {
    if (typeof PalmServiceBridge !== 'undefined') return;
    document.title = 'Prysm Capture · Simulated TV';
    const subscriptions = new Set<DevBridge>();
    const status: Status = {
        config: { receiver: '192.168.1.10:19400', source: 'both', compression: 'lz4', autostart: false, metrics: false },
        enabled: false, receiverConnected: false, capturing: false, senderRunning: false,
        stopping: false, power: 'Active', error: null, lastLog: 'Local development: simulated TV service.',
        capabilities: { hardwareH264: true },
    };
    class DevBridge implements ServiceBridge {
        onservicecallback = (_text: string) => {};
        cancel() { subscriptions.delete(this); }
        call(uri: string, payload: string) {
            const method = uri.slice(uri.lastIndexOf('/') + 1);
            const data = JSON.parse(payload);
            switch (method) {
                case 'status':
                    subscriptions.add(this);
                    break;
                case 'setConfig':
                    status.config = data as Config;
                    break;
                case 'start':
                case 'stop':
                case 'cleanup':
                    status.enabled = status.senderRunning = status.receiverConnected = status.capturing = method === 'start';
                    if (method === 'cleanup') status.config.autostart = false;
                    break;
                case 'exec':
                    this.onservicecallback(JSON.stringify({ returnValue: true }));
                    return;
                default:
                    this.onservicecallback(JSON.stringify({ returnValue: false, errorText: `Unknown method: ${method}` }));
                    return;
            }
            this.onservicecallback(JSON.stringify({ ...status, returnValue: true, subscribed: method === 'status' }));
            if (method !== 'status') {
                for (const bridge of subscriptions) {
                    bridge.onservicecallback(JSON.stringify({ ...status, returnValue: true, subscribed: true }));
                }
            }
        }
    }
    globalThis.PalmServiceBridge = DevBridge;
}
