export interface Config {
    receiver: string;
    source: 'both' | 'video';
    compression: 'lz4' | 'none' | 'h264-software' | 'h264-hardware';
    autostart: boolean;
    metrics: boolean;
}

export interface Status {
    config: Config;
    enabled: boolean;
    receiverConnected: boolean;
    capturing: boolean;
    videoAvailable?: boolean | null;
    senderRunning: boolean;
    stopping: boolean;
    power: string | null;
    error: string | null;
    lastLog: string | null;
    capabilities?: { hardwareH264?: boolean };
}

export type Reply = Partial<Status> & {
    returnValue?: boolean;
    subscribed?: boolean;
    errorText?: string;
    stderrString?: string;
};

export interface ServiceBridge {
    onservicecallback: (text: string) => void;
    call(uri: string, payload: string): void;
    cancel(): void;
}

declare global {
    var PalmServiceBridge: { new(): ServiceBridge };
}

const unavailable = 'Open Prysm Capture on your LG TV to connect to the service.';
const timedOut = 'The service did not respond. Open Service options to repair setup.';

export function parseReply(text: string): Reply {
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
        throw new Error('The service returned an invalid reply.');
    }
    return value as Reply;
}

function errorReply(error: unknown): Reply {
    return { returnValue: false, errorText: error instanceof Error ? error.message : String(error) };
}

// Keep native bridges alive until completion, scoped to the mounted consumer.
export function createServiceClient() {
    const pending = new Set<() => void>();
    function call(uri: string, payload: object, callback: (value: Reply) => void) {
        if (typeof PalmServiceBridge === 'undefined') {
            callback({ returnValue: false, errorText: unavailable });
            return;
        }
        let bridge: ServiceBridge;
        try { bridge = new PalmServiceBridge(); }
        catch (error) { callback(errorReply(error)); return; }
        let active = true;
        const timer = window.setTimeout(() => {
            cancel();
            callback({ returnValue: false, errorText: timedOut });
        }, 30000);
        function release() {
            active = false;
            clearTimeout(timer);
            pending.delete(cancel);
        }
        function cancel() {
            if (!active) return;
            release();
            bridge.cancel();
        }
        function done(value: Reply) {
            if (!active) return;
            release();
            callback(value);
        }
        pending.add(cancel);
        bridge.onservicecallback = text => {
            let value: Reply;
            try { value = parseReply(text); }
            catch (error) { value = errorReply(error); }
            done(value);
        };
        try { bridge.call(uri, JSON.stringify(payload)); }
        catch (error) { done(errorReply(error)); }
    }
    return { call, cancelAll: () => { for (const cancel of pending) cancel(); } };
}
