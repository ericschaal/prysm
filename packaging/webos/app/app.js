'use strict';

var bridges = [];
var statusBridge = null;
var statusRetry = null;
var statusTimer = null;
var busy = false;
var changing = false;
var online = null;
var observed = null;
var saved = null;
var draft = { receiver: '', source: 'both', compression: 'lz4', autostart: false, metrics: false };
var lastSetting = 'receiver';
var returnFocus = null;
var dialogBack = null;
var app = document.getElementById('app');
var modal = document.getElementById('modal');
var body = document.getElementById('dialog-body');
var names = {
    source: { both: 'Video + TV menus', video: 'Video only', ui: 'TV menus only' },
    compression: { lz4: 'LZ4', none: 'None', 'h264-software': 'H.264 (software)', 'h264-hardware': 'H.264 (hardware)' }
};
function element(id) { return document.getElementById(id); }
function copy(value) {
    return { receiver: value.receiver, source: value.source, compression: value.compression, autostart: value.autostart, metrics: value.metrics === true };
}
function dirty() { return !saved || JSON.stringify(draft) !== JSON.stringify(saved); }
function notice(text, error) {
    element('notice').textContent = text;
    element('notice').className = 'notice' + (error ? ' error' : '');
    element('notice').setAttribute('role', error ? 'alert' : 'status');
}
function call(uri, payload, callback) {
    if (typeof PalmServiceBridge === 'undefined') {
        callback({ returnValue: false, errorText: 'Open Prysm Capture on your LG TV to connect to the service.' });
        return;
    }
    var bridge = new PalmServiceBridge();
    bridges.push(bridge);
    var timer = setTimeout(function () {
        bridge.cancel();
        done({ returnValue: false, errorText: 'The service did not respond. Open Service options to repair setup.' });
    }, 30000);
    function done(value) {
        clearTimeout(timer);
        var index = bridges.indexOf(bridge);
        if (index < 0) return;
        bridges.splice(index, 1);
        callback(value);
    }
    bridge.onservicecallback = function (text) {
        var value;
        try { value = JSON.parse(text); }
        catch (error) { value = { returnValue: false, errorText: error.message }; }
        done(value);
    };
    bridge.call(uri, JSON.stringify(payload));
}
function observe(value) {
    online = true;
    observed = value;
    if (!saved) { saved = copy(value.config); draft = copy(saved); }
}
function stopWatching() {
    var bridge = statusBridge;
    statusBridge = null;
    clearTimeout(statusRetry);
    clearTimeout(statusTimer);
    if (bridge) bridge.cancel();
}
function watchStatus() {
    stopWatching();
    function failed(text) {
        stopWatching();
        online = false;
        notice(text || 'The service is unavailable. Open Service options to repair setup.', true);
        render();
        statusRetry = setTimeout(watchStatus, 3000);
    }
    if (typeof PalmServiceBridge === 'undefined') { failed('Open Prysm Capture on your LG TV to connect to the service.'); return; }
    var bridge = statusBridge = new PalmServiceBridge();
    statusTimer = setTimeout(function () { failed('The service did not respond. Open Service options to repair setup.'); }, 30000);
    bridge.onservicecallback = function (text) {
        if (statusBridge !== bridge) return;
        var value;
        try { value = JSON.parse(text); }
        catch (error) { failed(error.message); return; }
        if (!value || value.returnValue !== true || !value.config || value.subscribed !== true) {
            failed(value && value.errorText);
            return;
        }
        clearTimeout(statusTimer);
        if (online === false) notice('Service connected.');
        observe(value);
        render();
    };
    try { bridge.call('luna://io.prysm.capture.service/status', JSON.stringify({ subscribe: true })); }
    catch (error) { failed(error.message); }
}
function render() {
    var enabled = observed && observed.enabled;
    var connected = online && enabled && observed.receiverConnected;
    var ready = connected && observed.power === 'Active';
    document.body.classList.toggle('ready', !!ready);
    var connection = online === null ? 'Checking Prysm…' : online === false ? 'TV service unavailable' :
        observed.stopping ? 'Disconnecting…' : !enabled ? 'Sender disabled' :
        observed.power !== 'Active' ? 'Waiting for TV' : connected ? (observed.capturing ? 'Capturing' : 'Connected · Capture off') : 'Waiting for Prysm';
    element('connection').textContent = connection;
    element('connection').classList.toggle('offline', online === false || !!(enabled && !connected));
    element('connection').classList.toggle('inactive', online === null || !!(online && !enabled));
    var title = 'Getting ready';
    var description = 'Checking the capture service on this TV.';
    var label = 'Connecting…';
    if (online === false) {
        title = 'Let’s set up your TV';
        description = 'Set up the background service once, then connect it to Prysm.';
        label = 'Set up service';
    } else if (online) {
        if (observed.stopping) {
            title = 'Stopping capture';
            description = 'Waiting for capture to stop safely.';
        } else if (enabled && observed.power !== 'Active') {
            title = 'Waiting for the TV';
            description = 'The sender will be available when the TV is active.';
        } else if (enabled) {
            title = connected ? (observed.capturing ? 'Capturing' : 'Connected · Capture off') : 'Waiting for Prysm';
            description = connected ? 'Connected to ' + observed.config.receiver + '. ' + (observed.capturing ? 'Stop capture in Prysm.' : 'Start capture in Prysm when you’re ready.') :
                'No connection to ' + observed.config.receiver + '. Make sure Prysm is running with webOS input enabled.';
        } else {
            title = 'Sender disabled';
            description = 'Enable the sender here, then start capture in Prysm.';
        }
        if (observed.error) {
            title = 'Sender needs attention';
            description = 'Open Service options to view diagnostics or repair setup.';
        }
        label = enabled ? 'Disable sender' : !draft.receiver ? 'Set receiver address' : dirty() ? 'Save and enable' : 'Enable sender';
    }
    element('status-title').textContent = title;
    element('status-description').textContent = description;
    element('primary').textContent = changing ? 'Please wait…' : label;
    element('primary').disabled = online === null || changing;
    ['receiver', 'source', 'compression', 'autostart', 'metrics'].forEach(function (id) {
        element(id).disabled = !online || changing;
    });
    element('receiver-value').textContent = draft.receiver || 'Add receiver address';
    element('source-value').textContent = names.source[draft.source];
    element('compression-value').textContent = names.compression[draft.compression];
    element('autostart-value').textContent = draft.autostart ? 'On' : 'Off';
    element('metrics').setAttribute('aria-checked', String(draft.metrics));
    element('save').disabled = !online || !dirty() || changing;
    element('saved-state').textContent = saved ? dirty() ? 'Unsaved changes' : 'Saved' : 'On this TV';
    element('saved-state').classList.toggle('unsaved', !!saved && dirty());
    element('save-hint').textContent = dirty() && saved ? 'Choose Save changes to apply your settings.' : 'Changes apply only when you save.';
    if (modal.hidden && document.activeElement === document.body && !element('primary').disabled) element('primary').focus();
}
function request(method, payload, success) {
    if (busy) return;
    busy = true;
    changing = true;
    render();
    call('luna://io.prysm.capture.service/' + method, payload || {}, function (value) {
        busy = false;
        changing = false;
        if (value.returnValue !== true || !value.config) {
            notice(value.errorText || 'The service could not complete this action.', true);
            render();
            return;
        }
        observe(value);
        if (method === 'setConfig') saved = copy(value.config);
        if (method === 'cleanup') { saved = copy(value.config); draft.autostart = false; }
        render();
        if (success) success(value);
    });
}
function openDialog(title, description, build, back) {
    if (modal.hidden) returnFocus = document.activeElement;
    dialogBack = back || null;
    element('dialog-title').textContent = title;
    element('dialog-description').textContent = description;
    body.textContent = '';
    build(body);
    modal.hidden = false;
    app.setAttribute('aria-hidden', 'true');
    (body.querySelector('input, button') || element('dialog-back')).focus();
}
function closeDialog() {
    if (dialogBack) { dialogBack(); return; }
    modal.hidden = true;
    app.removeAttribute('aria-hidden');
    body.textContent = '';
    if (returnFocus && !returnFocus.disabled) returnFocus.focus();
    else if (!element('primary').disabled) element('primary').focus();
}
function button(container, id, title, detail, action) {
    var control = document.createElement('button');
    control.id = id;
    control.type = 'button';
    control.className = 'choice';
    var text = document.createElement('span');
    var name = document.createElement('span');
    name.className = 'choice-name';
    name.textContent = title;
    text.appendChild(name);
    if (detail) {
        var subtitle = document.createElement('span');
        subtitle.className = 'choice-detail';
        subtitle.textContent = detail;
        text.appendChild(subtitle);
    }
    control.appendChild(text);
    control.onclick = action;
    container.appendChild(control);
    return control;
}
function choices(key, title, description, values) {
    openDialog(title, description, function (container) {
        values.forEach(function (value) {
            var control = button(container, 'choice-' + String(value[0]), value[1], value[2], function () {
                draft[key] = value[0];
                closeDialog();
                render();
            });
            control.setAttribute('aria-pressed', String(draft[key] === value[0]));
            if (draft[key] === value[0]) {
                var check = document.createElement('span');
                check.className = 'check';
                check.textContent = '✓';
                check.setAttribute('aria-hidden', 'true');
                control.appendChild(check);
            }
        });
    });
    body.querySelector('[aria-pressed="true"]').focus();
}
function receiver() {
    openDialog('Prysm receiver', 'Enter the address of the computer running Prysm, including the port.', function (container) {
        var label = document.createElement('label');
        label.htmlFor = 'receiver-input';
        label.textContent = 'IP address and port';
        var input = document.createElement('input');
        input.id = 'receiver-input';
        input.type = 'text';
        input.value = draft.receiver;
        input.placeholder = '192.168.1.10:19400';
        input.spellcheck = false;
        input.autocomplete = 'off';
        input.setAttribute('autocapitalize', 'off');
        container.appendChild(label);
        container.appendChild(input);
        button(container, 'receiver-apply', 'Use this address', null, function () {
            draft.receiver = input.value.trim();
            closeDialog();
            render();
        });
    });
}
function saveSettings(success) {
    if (!draft.receiver) { notice('Add the Prysm receiver address first.', true); receiver(); return; }
    request('setConfig', copy(draft), function () { notice('Settings saved.'); if (success) success(); });
}
function setup() {
    if (busy) return;
    busy = changing = true;
    render();
    notice('Setting up the service. This may take a moment.');
    call('luna://org.webosbrew.hbchannel.service/exec', {
        command: '/media/developer/apps/usr/palm/services/io.prysm.capture.service/setup.sh'
    }, function (value) {
        busy = changing = false;
        if (value.returnValue === false || value.error || value.stderrString) {
            notice(value.errorText || value.error || value.stderrString, true);
            render();
        } else { notice('Service setup complete.'); watchStatus(); }
    });
}
function serviceOptions() {
    openDialog('Service options', 'Setup and troubleshooting. Your receiver settings are kept.', function (container) {
        button(container, 'repair', 'Set up or repair service', 'Register the background service and capture permissions.', function () { closeDialog(); setup(); });
        button(container, 'diagnostics', 'View diagnostics', 'Service state and the latest sender message.', function () {
            openDialog('Diagnostics', 'The sender being available does not mean Screen sync is active.', function (content) {
                var log = document.createElement('pre');
                log.className = 'diagnostics';
                log.textContent = observed ? 'TV power: ' + (observed.power || 'unknown') + '\nPrysm receiver connected: ' + !!observed.receiverConnected + '\nSender enabled: ' + observed.enabled +
                    '\nSender process running: ' + observed.senderRunning + '\n\n' + (observed.error || observed.lastLog || 'No errors reported.') : 'Service unavailable.';
                content.appendChild(log);
            }, serviceOptions);
        });
        button(container, 'cleanup', 'Remove service setup…', 'Stop the sender and remove its boot hook and capture permissions.', function () {
            openDialog('Remove service setup?', 'The sender will stop. Autostart and capture permissions will be removed. Your receiver settings are kept.', function (content) {
                button(content, 'keep-setup', 'Keep setup', null, serviceOptions);
                button(content, 'confirm-cleanup', 'Remove setup', null, function () {
                    dialogBack = null;
                    closeDialog();
                    request('cleanup', {}, function () { notice('Service setup removed. Receiver settings kept.'); });
                }).classList.add('danger');
            }, serviceOptions);
        });
    });
}
element('receiver').onclick = receiver;
element('source').onclick = function () {
    choices('source', 'What to capture', 'Choose a source, then save your changes on the main screen.', [
        ['both', 'Video + TV menus', 'Include the picture and on-screen menus.'],
        ['video', 'Video only', 'Capture the video picture.'],
        ['ui', 'TV menus only', 'Capture the TV interface.']
    ]);
};
element('compression').onclick = function () {
    var values = [
        ['lz4', 'LZ4', 'Lossless compression.'], ['none', 'None', 'Uncompressed frames. Uses more bandwidth.'], ['h264-software', 'H.264 (software)', 'Lossy compression using TV processing.']
    ];
    if (observed && observed.capabilities && observed.capabilities.hardwareH264 === true) {
        values.push(['h264-hardware', 'H.264 (hardware)', 'Uses the TV encoder. Falls back to software if unavailable.']);
    }
    choices('compression', 'Compression', 'Choose how frames are sent to Prysm.', values);
};
element('autostart').onclick = function () {
    choices('autostart', 'Enable with TV', 'Enable the background sender when the TV boots. Capture still follows Prysm.', [
        [false, 'Off', 'Enable the sender manually from this app.'], [true, 'On', 'Enable the sender when the TV boots.']
    ]);
};
element('save').onclick = function () { saveSettings(); };
element('metrics').onclick = function () {
    draft.metrics = !draft.metrics;
    render();
};
element('primary').onclick = function () {
    if (busy) return;
    if (!online) { setup(); return; }
    if (observed.enabled) { request('stop', {}, function () { notice('Sender disabled.'); }); return; }
    if (!draft.receiver) { receiver(); return; }
    if (dirty()) saveSettings(function () { request('start'); });
    else request('start');
};
element('service-options').onclick = serviceOptions;
element('dialog-back').onclick = closeDialog;
document.addEventListener('mouseover', function (event) {
    var control = event.target.closest('button');
    if (control && !control.disabled && (modal.hidden || modal.contains(control))) control.focus();
});
document.addEventListener('focusin', function (event) {
    if (event.target.classList.contains('setting')) lastSetting = event.target.id;
});
document.addEventListener('keydown', function (event) {
    var key = event.keyCode;
    if (key === 461 || key === 27) {
        event.preventDefault();
        if (!modal.hidden) closeDialog();
        else openDialog('Close Prysm Capture?', 'The sender keeps its current state when you close this app.', function (container) {
            button(container, 'stay', 'Stay here', null, closeDialog);
            button(container, 'exit', 'Close app', null, function () { window.close(); });
        });
        return;
    }
    if (!modal.hidden && key === 9) {
        var tabControls = Array.prototype.slice.call(modal.querySelectorAll('button, input'));
        var tabIndex = tabControls.indexOf(document.activeElement);
        event.preventDefault();
        tabControls[(tabIndex + (event.shiftKey ? tabControls.length - 1 : 1)) % tabControls.length].focus();
        return;
    }
    if ([37, 38, 39, 40].indexOf(key) < 0) return;
    if (document.activeElement.tagName === 'INPUT' && (key === 37 || key === 39)) return;
    event.preventDefault();
    if (!modal.hidden) {
        if (key === 37 || key === 39) return;
        var controls = Array.prototype.slice.call(modal.querySelectorAll('button, input'));
        var index = controls.indexOf(document.activeElement) + (key === 40 ? 1 : -1);
        if (index >= 0 && index < controls.length) controls[index].focus();
    } else {
        var direction = { 37: 'left', 38: 'up', 39: 'right', 40: 'down' }[key];
        var target = document.activeElement.getAttribute('data-' + direction);
        if (key === 39 && (document.activeElement.id === 'primary' || document.activeElement.id === 'service-options')) target = lastSetting;
        if (target && !element(target).disabled) element(target).focus();
    }
});
render();
watchStatus();
window.addEventListener('pagehide', stopWatching);
window.addEventListener('pageshow', watchStatus);
document.addEventListener('visibilitychange', function () { if (!document.hidden) watchStatus(); });
