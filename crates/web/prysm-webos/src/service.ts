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
