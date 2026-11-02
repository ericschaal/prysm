import { useEffect, useReducer, useRef, useState } from 'react';
import { createServiceClient, parseReply, type Config, type Status, type Reply, type ServiceBridge } from '../service';

const defaults: Config = { receiver: '', source: 'both', compression: 'lz4', autostart: false, metrics: false };
interface State {
    online: boolean | null;
    status: Status | null;
    saved: Config | null;
    draft: Config;
    changing: boolean;
    notice: string;
    error: boolean;
}
function normalizeConfig(value: Config): Config {
    return {
        receiver: value.receiver, source: value.source, compression: value.compression,
        autostart: value.autostart, metrics: value.metrics === true,
    };
}

type Action =
    | { type: 'observe'; status: Status; subscribed?: boolean }
    | { type: 'offline'; text: string }
    | { type: 'edit'; patch: Partial<Config> }
    | { type: 'notice'; text: string; error: boolean }
    | { type: 'changing'; value: boolean }
    | { type: 'saved'; config: Config; cleanup: boolean };

const initialState: State = {
    online: null, status: null, saved: null, draft: defaults,
    changing: false, notice: '', error: false,
};

function reducer(state: State, action: Action): State {
    switch (action.type) {
        case 'observe':
            return { ...state, online: true, status: action.status,
                saved: state.saved ?? normalizeConfig(action.status.config),
                draft: state.saved ? state.draft : normalizeConfig(action.status.config),
                notice: action.subscribed && state.online === false ? 'Service connected.' : state.notice,
                error: action.subscribed && state.online === false ? false : state.error,
            };
        case 'offline': return { ...state, online: false, error: true, notice: action.text };
        case 'edit': return { ...state, draft: { ...state.draft, ...action.patch } };
        case 'notice': return { ...state, notice: action.text, error: action.error };
        case 'changing': return { ...state, changing: action.value };
        case 'saved': return { ...state, saved: normalizeConfig(action.config),
            draft: action.cleanup ? { ...state.draft, autostart: false } : state.draft };
    }
}

export function useCaptureService(openReceiver: () => void) {
    const [client] = useState(createServiceClient);
    const call = client.call;
    const [state, dispatch] = useReducer(reducer, initialState);
    const busy = useRef(false);
    const reconnect = useRef(() => {});
    const { online, status: status, saved, draft } = state;
    const dirty = !saved || (Object.keys(draft) as (keyof Config)[]).some(key => draft[key] !== saved[key]);
    const enabled = status?.enabled;
    const notice = (text: string, error = false) => dispatch({ type: 'notice', text, error });
    const edit = <K extends keyof Config>(key: K, value: Config[K]) => dispatch({ type: 'edit', patch: { [key]: value } });

    useEffect(() => {
        let subscription: ServiceBridge | null = null;
        let retry: number | undefined;
        let timer: number | undefined;
        function stop() {
            const bridge = subscription;
            subscription = null;
            clearTimeout(retry);
            clearTimeout(timer);
            bridge?.cancel();
        }
        function failed(text?: string) {
            stop();
            dispatch({ type: 'offline', text: text || 'The service is unavailable. Open Service options to repair setup.' });
            retry = window.setTimeout(watch, 3000);
        }
        function watch() {
            stop();
            if (typeof PalmServiceBridge === 'undefined') {
                failed('Open Prysm Capture on your LG TV to connect to the service.');
                return;
            }
            let bridge: ServiceBridge;
            try { bridge = subscription = new PalmServiceBridge(); }
            catch (error) { failed(error instanceof Error ? error.message : String(error)); return; }
            timer = window.setTimeout(() => failed('The service did not respond. Open Service options to repair setup.'), 30000);
            bridge.onservicecallback = text => {
                if (subscription !== bridge) return;
                let value: Reply;
                try { value = parseReply(text); }
                catch (error) { failed(error instanceof Error ? error.message : String(error)); return; }
                if (!value || value.returnValue !== true || !value.config || value.subscribed !== true) {
                    failed(value?.errorText);
                    return;
                }
                clearTimeout(timer);
                dispatch({ type: 'observe', status: value as Status, subscribed: true });
            };
            try { bridge.call('luna://io.prysm.capture.service/status', JSON.stringify({ subscribe: true })); }
            catch (error) { failed(error instanceof Error ? error.message : String(error)); }
        }
        const visible = () => { if (!document.hidden) watch(); };
        reconnect.current = watch;
        watch();
        window.addEventListener('pagehide', stop);
        window.addEventListener('pageshow', watch);
        document.addEventListener('visibilitychange', visible);
        return () => {
            stop();
            window.removeEventListener('pagehide', stop);
            window.removeEventListener('pageshow', watch);
            document.removeEventListener('visibilitychange', visible);
            reconnect.current = () => {};
            client.cancelAll();
            busy.current = false;
        };
    }, [client]);

    function request(method: 'setConfig' | 'start' | 'stop' | 'cleanup', payload: object = {}, success?: () => void) {
        if (busy.current) return;
        busy.current = true;
        dispatch({ type: 'changing', value: true });
        call('luna://io.prysm.capture.service/' + method, payload, value => {
            busy.current = false;
            dispatch({ type: 'changing', value: false });
            if (value.returnValue !== true || !value.config) {
                notice(value.errorText || 'The service could not complete this action.', true);
                return;
            }
            dispatch({ type: 'observe', status: value as Status });
            if (method === 'setConfig' || method === 'cleanup') {
                dispatch({ type: 'saved', config: value.config, cleanup: method === 'cleanup' });
            }
            success?.();
        });
    }
    function saveSettings(success?: () => void) {
        if (!draft.receiver) { notice('Add the Prysm receiver address first.', true); openReceiver(); return; }
        request('setConfig', normalizeConfig(draft), () => { notice('Settings saved.'); success?.(); });
    }
    function setup() {
        if (busy.current) return;
        busy.current = true;
        dispatch({ type: 'changing', value: true });
        notice('Setting up the service. This may take a moment.');
        call('luna://org.webosbrew.hbchannel.service/exec', {
            command: '/media/developer/apps/usr/palm/services/io.prysm.capture.service/setup.sh'
        }, value => {
            busy.current = false;
            dispatch({ type: 'changing', value: false });
            if (value.returnValue === false || value.error || value.stderrString) {
                notice(value.errorText || value.error || value.stderrString || 'Service setup failed.', true);
            } else {
                notice('Service setup complete.');
                reconnect.current();
            }
        });
    }
    function primary() {
        if (busy.current) return;
        if (!online) { setup(); return; }
        if (enabled) { request('stop', {}, () => notice('')); return; }
        if (!draft.receiver) { openReceiver(); return; }
        if (dirty) saveSettings(() => request('start'));
        else request('start');
    }

    return { ...state, dirty, edit, saveSettings, primary, setup,
        cleanup: () => request('cleanup', {}, () => notice('Service setup removed. Receiver settings kept.')),
    };
}
