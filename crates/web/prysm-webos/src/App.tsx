import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { Config, Status, Reply, ServiceBridge } from './service';
const defaults: Config = { receiver: '', source: 'both', compression: 'lz4', autostart: false, metrics: false };
const names = {
    source: { both: 'Video + TV menus', video: 'Video only' },
    compression: { lz4: 'LZ4', none: 'None', 'h264-software': 'H.264 (software)', 'h264-hardware': 'H.264 (hardware)' }
};
type Dialog = 'receiver' | 'source' | 'compression' | 'autostart' | 'service' | 'diagnostics' | 'cleanup' | 'exit';
type ChoiceKey = 'source' | 'compression' | 'autostart';
type Choice<K extends ChoiceKey> = [Config[K], string, string];
interface State {
    online: boolean | null;
    observed: Status | null;
    saved: Config | null;
    draft: Config;
    changing: boolean;
    notice: string;
    error: boolean;
}
function copy(value: Config): Config {
    return {
        receiver: value.receiver, source: value.source, compression: value.compression,
        autostart: value.autostart, metrics: value.metrics === true,
    };
}
const bridges: ServiceBridge[] = [];
function call(uri: string, payload: object, callback: (value: Reply) => void) {
    if (typeof PalmServiceBridge === 'undefined') {
        callback({ returnValue: false, errorText: 'Open Prysm Capture on your LG TV to connect to the service.' });
        return;
    }
    const bridge = new PalmServiceBridge();
    bridges.push(bridge);
    const timer = setTimeout(() => {
        bridge.cancel();
        done({ returnValue: false, errorText: 'The service did not respond. Open Service options to repair setup.' });
    }, 30000);
    function done(value: Reply) {
        clearTimeout(timer);
        const index = bridges.indexOf(bridge);
        if (index < 0) return;
        bridges.splice(index, 1);
        callback(value);
    }
    bridge.onservicecallback = (text) => {
        let value: Reply;
        try { value = JSON.parse(text); }
        catch (error) { value = { returnValue: false, errorText: (error instanceof Error ? error.message : String(error)) }; }
        done(value);
    };
    bridge.call(uri, JSON.stringify(payload));
}

function ChoiceButton({ id, title, detail, action, selected, danger = false }: {
    id: string; title: string; detail?: string; action: () => void; selected?: boolean; danger?: boolean;
}) {
    return <button id={id} type="button" className={'choice' + (danger ? ' danger' : '')} onClick={action} aria-pressed={selected}>
        <span><span className="choice-name">{title}</span>{detail ? <span className="choice-detail">{detail}</span> : null}</span>
        {selected ? <span className="check" aria-hidden="true">✓</span> : null}
    </button>;
}

export default function App() {
    const [state, setState] = useState<State>({
        online: null, observed: null, saved: null, draft: defaults,
        changing: false, notice: '', error: false,
    });
    const [dialog, setDialog] = useState<Dialog | null>(null);
    const busy = useRef(false);
    const reconnect = useRef(() => {});
    const lastSetting = useRef('receiver');
    const returnFocus = useRef<HTMLElement | null>(null);
    const modal = useRef<HTMLDivElement>(null);
    const receiverInput = useRef<HTMLInputElement>(null);
    const { online, observed: status, saved, draft, changing } = state;
    const dirty = !saved || JSON.stringify(draft) !== JSON.stringify(saved);
    const enabled = status?.enabled;
    const connected = online && enabled && status?.receiverConnected;
    const ready = !!(connected && status?.power === 'Active');
    const notice = (text: string, error = false) => setState(current => ({ ...current, notice: text, error }));
    const edit = <K extends keyof Config>(key: K, value: Config[K]) => setState(current => ({ ...current, draft: { ...current.draft, [key]: value } }));
    function observe(value: Status) {
        setState(current => ({
            ...current, online: true, observed: value,
            saved: current.saved ?? copy(value.config),
            draft: current.saved ? current.draft : copy(value.config),
        }));
    }

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
            setState(current => ({ ...current, online: false, error: true,
                notice: text || 'The service is unavailable. Open Service options to repair setup.' }));
            retry = window.setTimeout(watch, 3000);
        }
        function watch() {
            stop();
            if (typeof PalmServiceBridge === 'undefined') {
                failed('Open Prysm Capture on your LG TV to connect to the service.');
                return;
            }
            const bridge = subscription = new PalmServiceBridge();
            timer = window.setTimeout(() => failed('The service did not respond. Open Service options to repair setup.'), 30000);
            bridge.onservicecallback = text => {
                if (subscription !== bridge) return;
                let value: Reply;
                try { value = JSON.parse(text); }
                catch (error) { failed(error instanceof Error ? error.message : String(error)); return; }
                if (!value || value.returnValue !== true || !value.config || value.subscribed !== true) {
                    failed(value?.errorText);
                    return;
                }
                clearTimeout(timer);
                setState(current => ({ ...current, online: true, observed: value as Status,
                    saved: current.saved ?? copy(value.config!),
                    draft: current.saved ? current.draft : copy(value.config!),
                    notice: current.online === false ? 'Service connected.' : current.notice,
                    error: current.online === false ? false : current.error,
                }));
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
            for (const bridge of bridges.splice(0)) bridge.cancel();
        };
    }, []);

    useEffect(() => {
        document.body.classList.toggle('ready', ready);
        return () => document.body.classList.remove('ready');
    }, [ready]);

    function openDialog(next: Dialog) {
        if (!dialog) returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        setDialog(next);
    }
    function closeDialog() {
        setDialog(dialog === 'diagnostics' || dialog === 'cleanup' ? 'service' : null);
    }
    useLayoutEffect(() => {
        if (dialog) {
            (modal.current?.querySelector<HTMLElement>('[aria-pressed="true"]') ?? modal.current?.querySelector<HTMLElement>('input, button'))?.focus();
        } else {
            const previous = returnFocus.current;
            if (previous && !(previous instanceof HTMLButtonElement && previous.disabled)) previous.focus();
        }
    }, [dialog]);
    useLayoutEffect(() => {
        if (!dialog && document.activeElement === document.body) document.getElementById('primary')?.focus();
    });

    function request(method: string, payload: object = {}, success?: () => void) {
        if (busy.current) return;
        busy.current = true;
        setState(current => ({ ...current, changing: true }));
        call('luna://io.prysm.capture.service/' + method, payload, value => {
            busy.current = false;
            setState(current => ({ ...current, changing: false }));
            if (value.returnValue !== true || !value.config) {
                notice(value.errorText || 'The service could not complete this action.', true);
                return;
            }
            observe(value as Status);
            if (method === 'setConfig' || method === 'cleanup') {
                const config = copy(value.config);
                setState(current => ({ ...current, saved: config,
                    draft: method === 'cleanup' ? { ...current.draft, autostart: false } : current.draft }));
            }
            success?.();
        });
    }
    function saveSettings(success?: () => void) {
        if (!draft.receiver) { notice('Add the Prysm receiver address first.', true); openDialog('receiver'); return; }
        request('setConfig', copy(draft), () => { notice('Settings saved.'); success?.(); });
    }
    function setup() {
        if (busy.current) return;
        busy.current = true;
        setState(current => ({ ...current, changing: true }));
        notice('Setting up the service. This may take a moment.');
        call('luna://org.webosbrew.hbchannel.service/exec', {
            command: '/media/developer/apps/usr/palm/services/io.prysm.capture.service/setup.sh'
        }, value => {
            busy.current = false;
            setState(current => ({ ...current, changing: false }));
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
        if (enabled) { request('stop', {}, () => notice('Sender disabled.')); return; }
        if (!draft.receiver) { openDialog('receiver'); return; }
        if (dirty) saveSettings(() => request('start'));
        else request('start');
    }

    const keydown = useEffectEvent((event: KeyboardEvent) => {
        const active = document.activeElement instanceof HTMLElement ? document.activeElement : document.body;
        const key = event.keyCode;
        if (key === 461 || key === 27) {
            event.preventDefault();
            if (dialog) closeDialog(); else openDialog('exit');
            return;
        }
        const controls = Array.from(modal.current?.querySelectorAll<HTMLElement>('button, input') ?? []);
        if (dialog && key === 9) {
            event.preventDefault();
            controls[(controls.indexOf(active) + (event.shiftKey ? controls.length - 1 : 1)) % controls.length]?.focus();
            return;
        }
        if ([37, 38, 39, 40].indexOf(key) < 0) return;
        if (active.tagName === 'INPUT' && (key === 37 || key === 39)) return;
        event.preventDefault();
        if (dialog) {
            if (key === 38 || key === 40) controls[controls.indexOf(active) + (key === 40 ? 1 : -1)]?.focus();
        } else {
            const direction = ({ 37: 'left', 38: 'up', 39: 'right', 40: 'down' } as Record<number, string>)[key];
            let target = active.getAttribute('data-' + direction);
            if (key === 39 && (active.id === 'primary' || active.id === 'service-options')) target = lastSetting.current;
            const control = target ? document.getElementById(target) : null;
            if (control instanceof HTMLButtonElement && !control.disabled) control.focus();
        }
    });
    useEffect(() => {
        const listener = (event: KeyboardEvent) => keydown(event);
        document.addEventListener('keydown', listener);
        return () => document.removeEventListener('keydown', listener);
    }, []);

    const connection = online === null ? 'Checking Prysm…' : online === false ? 'TV service unavailable' : !status ? 'Checking Prysm…' :
        status.stopping ? 'Disconnecting…' : !enabled ? 'Sender disabled' :
        status.power !== 'Active' ? 'Waiting for TV' : connected ? (status.capturing ? 'Capturing' : 'Connected · Capture off') : 'Waiting for Prysm';
    let title = 'Getting ready';
    let description = 'Checking the capture service on this TV.';
    let label = 'Connecting…';
    if (online === false) {
        title = 'Let’s set up your TV';
        description = 'Set up the background service once, then connect it to Prysm.';
        label = 'Set up service';
    } else if (online && status) {
        if (status.stopping) {
            title = 'Stopping capture'; description = 'Waiting for capture to stop safely.';
        } else if (enabled && status.power !== 'Active') {
            title = 'Waiting for the TV'; description = 'The sender will be available when the TV is active.';
        } else if (enabled) {
            title = connected ? (status.capturing ? 'Capturing' : 'Connected · Capture off') : 'Waiting for Prysm';
            description = connected ? 'Connected to ' + status.config.receiver + '. ' + (status.capturing ? 'Stop capture in Prysm.' : 'Start capture in Prysm when you’re ready.') :
                'No connection to ' + status.config.receiver + '. Make sure Prysm is running with webOS input enabled.';
        } else {
            title = 'Sender disabled'; description = 'Enable the sender here, then start capture in Prysm.';
        }
        if (status.error) { title = 'Sender needs attention'; description = 'Open Service options to view diagnostics or repair setup.'; }
        label = enabled ? 'Disable sender' : !draft.receiver ? 'Set receiver address' : dirty ? 'Save and enable' : 'Enable sender';
    }

    let dialogTitle = '';
    let dialogDescription = '';
    let content: ReactNode = null;
    function choices<K extends ChoiceKey>(key: K, values: Choice<K>[]) {
        return values.map(([value, name, detail]) => <ChoiceButton key={String(value)} id={'choice-' + value} title={name} detail={detail}
            selected={draft[key] === value} action={() => { edit(key, value); closeDialog(); }} />);
    }
    switch (dialog) {
        case 'receiver':
            dialogTitle = 'Prysm receiver';
            dialogDescription = 'Enter the address of the computer running Prysm, including the port.';
            content = <><label htmlFor="receiver-input">IP address and port</label>
                <input ref={receiverInput} id="receiver-input" type="text" defaultValue={draft.receiver} placeholder="192.168.1.10:19400" spellCheck={false} autoComplete="off" autoCapitalize="off" />
                <ChoiceButton id="receiver-apply" title="Use this address" action={() => { edit('receiver', receiverInput.current!.value.trim()); closeDialog(); }} /></>;
            break;
        case 'source':
            dialogTitle = 'What to capture'; dialogDescription = 'Choose a source, then save your changes on the main screen.';
            content = choices('source', [['both', 'Video + TV menus', 'Include the picture and on-screen menus.'], ['video', 'Video only', 'Capture the video picture.']]);
            break;
        case 'compression': {
            dialogTitle = 'Compression'; dialogDescription = 'Choose how frames are sent to Prysm.';
            const values: Choice<'compression'>[] = [['lz4', 'LZ4', 'Lossless compression.'], ['none', 'None', 'Uncompressed frames. Uses more bandwidth.'], ['h264-software', 'H.264 (software)', 'Lossy compression using TV processing.']];
            if (status?.capabilities?.hardwareH264 === true) values.push(['h264-hardware', 'H.264 (hardware)', 'Uses the TV encoder. Falls back to software if unavailable.']);
            content = choices('compression', values);
            break;
        }
        case 'autostart':
            dialogTitle = 'Enable with TV'; dialogDescription = 'Enable the background sender when the TV boots. Capture still follows Prysm.';
            content = choices('autostart', [[false, 'Off', 'Enable the sender manually from this app.'], [true, 'On', 'Enable the sender when the TV boots.']]);
            break;
        case 'service':
            dialogTitle = 'Service options'; dialogDescription = 'Setup and troubleshooting. Your receiver settings are kept.';
            content = <>
                <ChoiceButton id="repair" title="Set up or repair service" detail="Register the background service and capture permissions." action={() => { closeDialog(); setup(); }} />
                <ChoiceButton id="diagnostics" title="View diagnostics" detail="Service state and the latest sender message." action={() => openDialog('diagnostics')} />
                <ChoiceButton id="cleanup" title="Remove service setup…" detail="Stop the sender and remove its boot hook and capture permissions." action={() => openDialog('cleanup')} />
            </>;
            break;
        case 'diagnostics':
            dialogTitle = 'Diagnostics'; dialogDescription = 'The sender being available does not mean Screen sync is active.';
            content = <pre className="diagnostics">{status ? `TV power: ${status.power || 'unknown'}
Prysm receiver connected: ${!!status.receiverConnected}
Sender enabled: ${status.enabled}
Sender process running: ${status.senderRunning}

${status.error || status.lastLog || 'No errors reported.'}` : 'Service unavailable.'}</pre>;
            break;
        case 'cleanup':
            dialogTitle = 'Remove service setup?'; dialogDescription = 'The sender will stop. Autostart and capture permissions will be removed. Your receiver settings are kept.';
            content = <><ChoiceButton id="keep-setup" title="Keep setup" action={() => setDialog('service')} />
                <ChoiceButton id="confirm-cleanup" title="Remove setup" danger action={() => { setDialog(null); request('cleanup', {}, () => notice('Service setup removed. Receiver settings kept.')); }} /></>;
            break;
        case 'exit':
            dialogTitle = 'Close Prysm Capture?'; dialogDescription = 'The sender keeps its current state when you close this app.';
            content = <><ChoiceButton id="stay" title="Stay here" action={closeDialog} /><ChoiceButton id="exit" title="Close app" action={() => window.close()} /></>;
    }

    return <div onMouseOver={event => {
        const control = event.target instanceof Element ? event.target.closest('button') : null;
        if (control && !control.disabled && (!dialog || modal.current?.contains(control))) control.focus();
    }} onFocus={event => { if (event.target.classList.contains('setting')) lastSetting.current = event.target.id; }}>
        <main id="app" aria-hidden={dialog ? true : undefined}>
            <header className="header">
                <div className="brand"><svg className="logo" viewBox="0 0 32 32" fill="none" aria-hidden="true"><path d="m16 3 13 24H3L16 3Z" stroke="currentColor" strokeWidth="2" /><path d="m16 3 4 24M3 27l15-12 11 12" stroke="currentColor" strokeWidth="1.5" /></svg><div><h1>prysm<span className="brand-dot">.</span></h1><p>TV capture</p></div></div>
                <p id="connection" className={'connection' + (online === false || (enabled && !connected) ? ' offline' : '') + (online === null || (online && !enabled) ? ' inactive' : '')}>{connection}</p>
            </header>
            <div className="layout">
                <section className="overview" aria-labelledby="status-title">
                    <p className="eyebrow">TV CAPTURE</p><h2 id="status-title">{title}</h2><p id="status-description" className="description">{description}</p>
                    <div className="light-spectrum" aria-hidden="true"><span /><span /><span /><span /><span /></div>
                    <div className="main-actions">
                        <button id="primary" className="primary" data-down="service-options" data-right="receiver" disabled={online === null || changing} onClick={primary}>{changing ? 'Please wait…' : label}</button>
                        <button id="service-options" className="quiet" data-up="primary" data-right="receiver" onClick={() => openDialog('service')}>Service options</button>
                    </div>
                </section>
                <section className="settings" aria-labelledby="settings-title">
                    <div className="section-heading"><h2 id="settings-title">Settings</h2><span id="saved-state" className={saved && dirty ? 'unsaved' : undefined}>{saved ? dirty ? 'Unsaved changes' : 'Saved' : 'On this TV'}</span></div>
                    <div className="setting-list">
                        {(['receiver', 'source', 'compression', 'autostart'] as const).map((key, index, keys) => <button key={key} id={key} className="setting" aria-haspopup="dialog" data-up={keys[index - 1] ?? 'receiver'} data-down={keys[index + 1] ?? 'metrics'} data-left="primary" disabled={!online || changing} onClick={() => openDialog(key)}>
                            <span><span className="setting-label">{{ receiver: 'Prysm receiver', source: 'Capture', compression: 'Compression', autostart: 'Enable with TV' }[key]}</span>
                                <span id={key + '-value'} className={'setting-value' + (key === 'receiver' ? ' address' : '')}>{key === 'receiver' ? draft.receiver || 'Add receiver address' : key === 'source' ? names.source[draft.source] : key === 'compression' ? names.compression[draft.compression] : draft.autostart ? 'On' : 'Off'}</span></span><span className="chevron" aria-hidden="true">›</span>
                        </button>)}
                        <button id="metrics" className="setting" role="checkbox" aria-checked={draft.metrics} aria-describedby="metrics-description" data-up="autostart" data-down="save" data-left="primary" disabled={!online || changing} onClick={() => edit('metrics', !draft.metrics)}>
                            <span><span className="setting-label">Prometheus metrics</span><span id="metrics-description" className="setting-value">Expose TV metrics on port 9101 while the sender runs.</span></span><span className="tickbox" aria-hidden="true" />
                        </button>
                    </div>
                    <div className="settings-footer"><p id="save-hint">{dirty && saved ? 'Choose Save changes to apply your settings.' : 'Changes apply only when you save.'}</p><button id="save" className="secondary" data-up="metrics" data-left="service-options" disabled={!online || !dirty || changing} onClick={() => saveSettings()}>Save changes</button></div>
                </section>
            </div>
            <p id="notice" className={'notice' + (state.error ? ' error' : '')} role={state.error ? 'alert' : 'status'} aria-live="polite">{state.notice}</p>
            <footer className="key-help"><span><b>↑ ↓ ← →</b> Move</span><span><b>OK</b> Choose</span><span><b>Back</b> Return</span><span className="panel-hint">Start and stop capture in Prysm.</span></footer>
        </main>
        <div ref={modal} id="modal" className="modal" hidden={!dialog}>
            <section className="dialog" role="dialog" aria-modal="true" aria-labelledby="dialog-title" aria-describedby="dialog-description">
                <p className="eyebrow">PRYSM CAPTURE</p><h2 id="dialog-title">{dialogTitle}</h2><p id="dialog-description" className="dialog-description">{dialogDescription}</p>
                <div id="dialog-body" key={dialog}>{content}</div><button id="dialog-back" className="quiet dialog-back" onClick={closeDialog}>Back</button>
            </section>
        </div>
    </div>;
}
