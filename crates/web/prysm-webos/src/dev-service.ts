import type { Config, Status, ServiceBridge } from './service';

// In-memory Luna simulation for the Vite dev server; removed from TV builds.
export function installDevService(embeddedPortal = false) {
  if (typeof PalmServiceBridge !== 'undefined') return;
  document.title = 'Prysm Capture · Simulated TV';
  const subscriptions = new Set<DevBridge>();
  const status: Status = {
    returnValue: true,
    subscribed: false,
    revision: 0,
    subscribers: 0,
    pid: 0,
    config: {
      instanceUrl: 'http://192.168.1.10:8787',
      source: 'both',
      compression: 'lz4',
      autostart: false,
      metrics: false,
    },
    enabled: false,
    receiverBusy: false,
    receiverConnected: false,
    capturing: false,
    senderRunning: false,
    stopping: false,
    videoAvailable: true,
    power: 'Active',
    error: null,
    lastLog: 'Local development: simulated TV service.',
    capabilities: {
      hardwareH264: true,
      ...(embeddedPortal ? { embeddedPortal: true } : {}),
    },
  };
  let remoteEnabled = false;
  class DevBridge implements ServiceBridge {
    onservicecallback: (text: string) => void = () => {};
    cancel() {
      subscriptions.delete(this);
    }
    call(uri: string, payload: string) {
      const method = uri.slice(uri.lastIndexOf('/') + 1);
      const data: unknown = JSON.parse(payload);
      switch (method) {
        case 'status':
          subscriptions.add(this);
          break;
        case 'setConfig': {
          const wasEmbedded = status.config.location === 'embedded';
          status.config = data as Config;
          if (status.config.location === 'embedded') {
            if (!wasEmbedded) {
              remoteEnabled = status.enabled;
              status.enabled = true;
            }
            status.senderRunning = true;
            status.receiverConnected = false;
            status.capturing = false;
            status.portalUrl = 'http://localhost:8787/';
          } else if (wasEmbedded) {
            delete status.portalUrl;
            status.senderRunning =
              status.enabled =
              status.receiverConnected =
              status.capturing =
                remoteEnabled;
          }
          break;
        }
        case 'start':
        case 'stop':
        case 'cleanup':
          status.enabled = method === 'start';
          status.senderRunning =
            status.enabled ||
            (status.config.location === 'embedded' && method !== 'cleanup');
          status.capturing =
            status.enabled && status.config.location !== 'embedded';
          status.receiverConnected =
            status.enabled && status.config.location !== 'embedded';
          if (method === 'cleanup') {
            status.config.autostart = false;
            delete status.config.location;
            delete status.portalUrl;
          }
          break;
        case 'exec':
          this.onservicecallback(JSON.stringify({ returnValue: true }));
          return;
        default:
          this.onservicecallback(
            JSON.stringify({
              returnValue: false,
              errorText: `Unknown method: ${method}`,
            }),
          );
          return;
      }
      this.onservicecallback(
        JSON.stringify({
          ...status,
          returnValue: true,
          subscribed: method === 'status',
        }),
      );
      if (method !== 'status') {
        for (const bridge of subscriptions) {
          bridge.onservicecallback(
            JSON.stringify({ ...status, returnValue: true, subscribed: true }),
          );
        }
      }
    }
  }
  globalThis.PalmServiceBridge = DevBridge;
}
