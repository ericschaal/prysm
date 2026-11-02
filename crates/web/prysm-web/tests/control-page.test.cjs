// Run after npm ci in crates/web/prysm-web/control-ui: node crates/web/prysm-web/tests/control-page.test.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('../control-ui/node_modules/jsdom');
const dist = path.join(__dirname, '../control-ui/dist');
const html = fs.readFileSync(path.join(dist, 'index.html'), 'utf8');
const scriptPath = html.match(/<script[^>]+src="([^"]+)"/)[1];
const script = fs.readFileSync(path.join(dist, scriptPath), 'utf8');
const processing = { brightness_percent: 100, shadow_lift_percent: 0, smoothing_seconds: 0.1, sample_density: 10, spatial_reach_percent: 10, remove_black_bars: true };
const revision = '1';
const operationId = '00000000-0000-4000-8000-000000000002';
function config(lift = 0, revision = '1') {
  return { revision, source: 'prysm', web: { listen: '127.0.0.1:8787', allowed_hosts: ['localhost:8787'] },
    input: { type: 'device', device: '/dev/video0', refresh_rate_hz: 30, capture_resolution: [640, 360] },
    output: { type: 'wled', url: 'http://192.168.18.39/', leds: { top: 10, right: 10, bottom: 10, left: 10 } },
    processing: { ...processing, shadow_lift_percent: lift } };
}
const response = (data, status = 200) => new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } });
const off = { source: 'prysm', observed: { power: false, realtime: false, live_override: 2,
  pipeline_phase: 'idle' }, operation: { kind: 'idle' }, config_revision: revision, applied_revision: null, restart_required: false, pipeline: { phase: 'idle', applied_revision: null, frames_processed: 0, error: null },
  observation_error: null, wled_url: 'http://192.168.18.39/' };
const on = { ...off, observed: { ...off.observed, power: true, realtime: true,
  live_override: 0, pipeline_phase: 'running' }, pipeline:{...off.pipeline,phase:'running'}, applied_revision: revision };
const wledOn = { ...off, source: 'wled', observed: { ...off.observed, power: true } };
const offline = { ...off, observed: null, observation_error: 'WLED offline' };
const delay = () => new Promise(resolve => setTimeout(resolve, 5));
async function until(check) {
  for (let i = 0; i < 400; i++) {
    if (check()) return;
    await delay();
  }
  assert.fail('Timed out waiting for UI');
}

function page(initial = off, initialConfig = config()) {
  const state = { data: initial, config: initialConfig, configGets: 0, configGet: null, requests: [], configRequests: [], configPut: null, sockets: [], draws: [], failure: null, hidden: false, put: null, get: null };
  const dom = new JSDOM(html, {
    url: 'http://localhost:8787/', runScripts: 'dangerously', pretendToBeVisual: true,
    beforeParse(window) {
      window.AbortController = AbortController;
      window.AbortSignal = AbortSignal;
      window.Request = Request;
      window.Response = Response;
      window.Headers = Headers;
      window.WebSocket = class {
        static OPEN = 1;
        readyState = 1;
        sent = [];
        closed = false;
        constructor(url) { this.url = url.toString(); state.sockets.push(this); window.setTimeout(() => this.onopen?.(), 0); }
        send(value) { assert.equal(value, 'next'); this.sent.push(value); }
        close() { this.closed = true; this.readyState = 3; }
        message(value) { return this.onmessage?.({ data: JSON.stringify(value) }); }
      };
      window.Image = class { async decode() { if (state.decode) await state.decode(); } };
      window.HTMLCanvasElement.prototype.getContext = () => ({ fillRect(...args) { state.draws.push(['fill', this.fillStyle, ...args]); }, drawImage(...args) { state.draws.push(['image', ...args.slice(1)]); }, strokeRect(...args) { state.draws.push(['crop', ...args]); }, setLineDash() {} });
      Object.defineProperty(window.document, 'hidden', { get: () => state.hidden });
      window.setInterval = (callback, ms) => { assert.equal(ms, 2000); state.poll = callback; return 1; };
      window.clearInterval = () => {};
      const schedule = window.setTimeout.bind(window);
      window.setTimeout = (callback, ms, ...args) => { if (ms === 10000) state.previewTimeout = callback; return schedule(callback, ms, ...args); };
      window.fetch = async (request) => {
        const url = new URL(request.url).pathname;
        const options = { method: request.method, headers: Object.fromEntries(request.headers), body: request.method === 'PUT' ? await request.text() : undefined };
        if (url === '/api/config') {
          if (options.method === 'GET') {
            state.configGets += 1;
            if (state.configGet) return state.configGet();
            if (state.failure) throw Error(state.failure);
            return response(state.config);
          }
          assert.equal(options.method, 'PUT');
          const update = JSON.parse(options.body);
          if (!state.configPut) assert.equal(update.expected_revision, state.config.revision);
          assert.deepEqual(update.config.web, state.config.web);
          assert.deepEqual(update.config.output, state.config.output);
          assert.deepEqual(update.config.input, state.config.input);
          state.configRequests.push(update.config);
          if (state.configPut) return state.configPut(update.config);
          const nextRevision = String(Number(state.config.revision) + 1);
          state.config = { ...update.config, revision: nextRevision };
          state.data = { ...state.data, config_revision: nextRevision };
          return response(state.config);
        }
        assert.equal(url, '/api/state');
        state.requests.push(options);
        if (options.method === 'PUT') {
          if (state.put) return state.put(options);
          state.data = { ...state.data, operation: { kind: 'pending', id: operationId } };
          const operation_id = operationId;
          return response({ operation_id }, 202);
        }
        if (state.get) return state.get();
        if (state.failure) throw Error(state.failure);
        return response(state.data);
      };
    },
  });
  dom.window.eval(script);
  state.dom = dom;
  state.document = dom.window.document;
  state.element = selector => state.document.querySelector(selector);
  state.refresh = async (data, config) => {
    if (data) state.data = data;
    if (config) state.config = config;
    await state.poll();
    await delay();
  };
  state.ready = () => until(() => state.element('.connection') && !state.element('.connection').textContent.includes('Connecting'));
  return state;
}

async function check(initial, test, initialConfig) {
  const state = page(initial, initialConfig);
  try { await state.ready(); await test(state); }
  finally { await delay(); state.dom.window.close(); }
}

function button(p, text) { return [...p.document.querySelectorAll('button, .main-nav a')].find(element => (element.getAttribute('aria-label') || element.textContent) === text); }
function tune(p) { button(p, 'Preview & tuning ').click(); }
function change(p, selector, value) {
  const input = p.element(selector);
  Object.getOwnPropertyDescriptor(p.dom.window.HTMLInputElement.prototype, 'value').set.call(input, String(value));
  input.dispatchEvent(new p.dom.window.Event('input', { bubbles: true }));
}
const frame = { type: 'frame', frame_id: 9, revision: '1', source_width: 640, source_height: 360, width: 480, height: 270,
  viewport: { x: 0, y: 20, width: 640, height: 320 }, colors: { top: [[255,0,0]], right: [[0,255,0]], bottom: [[255,0,0],[0,0,255]], left: [[0,255,0],[255,255,0]] }, jpeg: 'test' };
(async () => {
  await check(on, async p => {
    assert.equal(p.element('.setup-diagnostics').textContent.includes('0.0.0.0'),false,'A listening address is not a video source');
    assert.deepEqual([...p.document.querySelectorAll('.setup-diagnostics dt')].map(e=>e.textContent),['Light controller']);
    assert.match(p.element('.setup-diagnostics dd').textContent,/192\.168\.18\.39/);
  }, {...config(),input:{type:'webos',listen:'0.0.0.0:19400',refresh_rate_hz:30,capture_resolution:[640,360]}});
  await check(wledOn, async p => {
    assert.equal(p.element('.source-options').dataset.selected,'wled');
    assert.equal(p.element('#wled').textContent,'Open WLED ↗');
    assert.equal(p.element('[data-source="wled"]').getAttribute('aria-pressed'),'true');
  });
  await check(on, async p => {
    assert.equal(p.sockets.length, 0, 'Overview does no preview work');
    assert.match(p.element('.led-layout svg title').textContent, /10 top, 10 right, 10 bottom, 10 left/);
    assert.match(p.element('.screen-label').textContent, /40 LEDs/);
    assert.equal(p.element('.led-layout figcaption'),null);
    assert.equal(p.element('.overview-grid').children.length,2,'Overview has one controls card and one setup card');
    assert.equal(p.element('.power-panel .diagnostics') !== null,true,'Diagnostics stay with the controls');
    assert.deepEqual([...p.document.querySelectorAll('.power-diagnostics dt')].map(e=>e.textContent),['Capture','Last change','Frames processed','Settings revision']);
    assert.equal(p.element('.power-diagnostics dd').textContent,'Capturing');
    assert.match(p.element('.revision-values').textContent,/1 saved1 applied/);
    assert.equal(p.element('[data-source="prysm"]').textContent.trim(),'Screen sync');
    assert.equal(p.element('.source-options').dataset.selected,'prysm');
    assert.equal(p.element('#wled'),null,'WLED link stays with the WLED source');
    assert.ok(p.element('.setup-flow').compareDocumentPosition(p.element('.led-layout')) & 4,'Signal path comes before the LED layout');
    assert.equal(p.element('footer'),null);
    assert.equal(p.element('#remove_black_bars').getAttribute('role'),'switch');
    assert.ok(p.element('.picture-settings #brightness_percent'));
    assert.ok(p.element('.picture-settings #smoothing_seconds'));
    assert.ok(p.element('.advanced #remove_black_bars'));
    assert.equal(p.element('#color-heading').textContent,'Color & motion');
    assert.equal(p.element('label[for="remove_black_bars"]').textContent,'Remove black bars');
    assert.equal(p.element('.diagnostics').open, false, 'Internal counters stay behind disclosure');
    assert.doesNotMatch(p.element('.settings').textContent, /4,294,967,295|3.402823|Range:/);
    assert.equal(p.document.querySelectorAll('nav a').length, 2);
    tune(p); await delay();
    assert.equal(p.document.querySelectorAll('.settings input').length, 6);
    assert.equal(p.sockets.length, 0, 'Tuning page also starts with preview closed');
    for (const [key, value] of Object.entries({brightness_percent: 65, shadow_lift_percent: 25, smoothing_seconds: 0.8, sample_density: 60, spatial_reach_percent: 22})) change(p, '#' + key, value);
    p.element('#remove_black_bars').click(); await delay();
    assert.match(p.element('#settings-status').textContent, /Unsaved/);
    assert.equal(p.configRequests.length, 0, 'Editing never autosaves competing full configs');
    button(p, 'Overview').click(); await delay();
    assert.ok(p.element('.draft-dot'), 'Navigation keeps and marks the tuning draft');
    tune(p); await delay();
    assert.equal(p.element('#brightness_percent').value, '65');
    button(p, 'Save & apply').click();
    await until(() => p.configRequests.length === 1); await delay();
    assert.deepEqual(p.configRequests[0].processing, {brightness_percent:65, shadow_lift_percent:25, smoothing_seconds:0.8, sample_density:60, spatial_reach_percent:22, remove_black_bars:false});
    assert.equal(p.requests.filter(r => r.method === 'PUT').length, 0, 'Live tuning issues no power or restart request');
    assert.match(p.element('#settings-status').textContent, /Saved/);
    await p.refresh({ ...p.data, applied_revision: '2', pipeline: {...p.data.pipeline, applied_revision:'2', phase:'running'} });
    assert.match(p.element('#settings-status').textContent, /Applied to your lights/);
  });
  await check(on, async p => {
    tune(p); await delay(); change(p, '#brightness_percent', 40); await delay();
    button(p, 'Discard').click(); await delay();
    assert.equal(p.element('#brightness_percent').value, '100');
    assert.equal(button(p, 'Save & apply').disabled, true);
    button(p, 'Disable smoothing').click(); await delay();
    assert.equal(p.element('#smoothing_seconds').value, '0');
    change(p, '#sample_density', 0); await delay();
    assert.equal(p.element('#sample_density').getAttribute('aria-invalid'), 'true');
    assert.ok(p.element('#sample_density-error'));
    assert.equal(button(p, 'Save & apply').disabled, true);
    change(p, '#sample_density', 12); await delay();
    assert.equal(button(p, 'Save & apply').disabled, false);
    p.configPut = async () => response({code:'invalid_request',message:'Server rejected settings'},422);
    button(p, 'Save & apply').click();
    await until(() => /Server rejected/.test(p.element('.settings').textContent));
    assert.equal(p.element('#sample_density').value, '12');
    assert.match(p.element('.settings').textContent, /draft is preserved/);
    p.configPut = null;
    button(p, 'Save & apply').click(); await until(() => p.configRequests.length === 2);
  });
  await check(on, async p => {
    tune(p); await delay(); change(p, '#shadow_lift_percent', 35); await delay();
    await p.refresh({ ...on, config_revision:'2' }, { ...config(0,'2'), processing:{...processing,brightness_percent:42} });
    assert.equal(p.element('#shadow_lift_percent').value, '35');
    assert.match(p.element('.settings').textContent, /changed elsewhere/);
    assert.match(p.element('.conflict-values').textContent, /Saved: 42/);
    assert.equal(button(p,'Save & apply').disabled,true);
    button(p,'Reload saved settings & discard draft').click(); await delay();
    assert.equal(p.element('#brightness_percent').value,'42');
    assert.equal(p.element('#shadow_lift_percent').value,'0');
  });
  await check(on, async p => {
    tune(p); await delay(); change(p, '#shadow_lift_percent', 35); await delay();
    p.configPut = async () => response({code:'conflict',message:'Refresh before saving'},409);
    button(p,'Save & apply').click(); await until(() => /Refresh before saving/.test(p.element('.settings').textContent));
    assert.equal(p.element('#shadow_lift_percent').value,'35');
    assert.equal(button(p,'Save & apply').disabled,true,'409 requires explicit reload, never blind retry');
  });
  await check(on, async p => {
    tune(p); await delay(); change(p, '#brightness_percent', 20); await delay();
    p.failure='Network unavailable'; await p.refresh();
    assert.equal(p.element('#brightness_percent').value,'20');
    assert.equal(button(p,'Save & apply').disabled,true);
    p.failure=null; await p.refresh({...on,restart_required:true});
    assert.equal(p.element('#power').disabled,true);
    assert.match(p.element('.tuning-grid').textContent,/application restart/);
    assert.equal(button(p,'Save & apply').disabled,false,'Live settings are still editable');
    p.configPut=async()=>{p.config={...p.config,revision:'2'};p.data={...p.data,config_revision:'2',operation:{kind:'failed',id:operationId,error:{code:'backend',message:'Apply failed'}}};return response(p.config);};
    button(p,'Save & apply').click(); await until(()=>/Apply failed/.test(p.element('.tuning-grid').textContent));
    assert.match(p.element('.tuning-grid').textContent,/Saved values may differ/);
  });
  await check(off, async p => {
    tune(p); await delay();
    assert.match(p.element('.preview-note').textContent,/Lights are off/);
    assert.equal(p.sockets.length,0,'The note does not open preview or change lighting');
    assert.equal(button(p,'Open preview').matches(':disabled'),true);
    button(p,'Open preview').click(); await delay();
    assert.equal(p.sockets.length,0,'Preview cannot be opened while the lights are off');
    await p.refresh(wledOn);
    assert.match(p.element('.preview-note').textContent,/WLED effects are selected/);
    assert.equal(button(p,'Open preview').matches(':disabled'),true);
    await p.refresh({...on,pipeline:{...on.pipeline,phase:'idle'}});
    assert.match(p.element('.preview-note').textContent,/Screen sync is stopped/);
    await p.refresh({...on,pipeline:{...on.pipeline,phase:'running'}});
    assert.equal(p.element('.preview-note'),null,'No warning while Screen sync is running');
    assert.equal(p.requests.filter(r=>r.method==='PUT').length,0);
  });
  await check(on, async p => {
    tune(p); await delay(); button(p,'Open preview').click();
    await until(()=>p.sockets.length===1 && p.sockets[0].sent.length===1);
    let finishDecode; p.decode=()=>new Promise(resolve=>{finishDecode=resolve;});
    const pending=p.sockets[0].message(frame); await delay();
    await p.refresh(off);
    assert.equal(p.sockets[0].closed,true,'Power off tears down preview transport');
    assert.equal(button(p,'Open preview').matches(':disabled'),true);
    assert.equal(p.element('.preview .badge').textContent,'Unavailable');
    finishDecode(); await pending;
    assert.equal(p.draws.length,0,'An in-flight decode cannot draw after power off');
    await p.refresh(on);
    assert.equal(button(p,'Open preview').matches(':disabled'),false);
    assert.equal(p.sockets.length,1,'Power on does not reopen preview automatically');
    assert.equal(p.element('.preview .badge').textContent,'Closed');
  });
  await check(off, async p => {
    p.element('#power').click(); p.element('#power').click();
    await until(()=>p.requests.some(r=>r.method==='PUT')); await delay();
    assert.equal(p.requests.filter(r=>r.method==='PUT').length,1);
    assert.equal(p.element('.power-heading h2').textContent,'Off','Keep the observed state steady while waiting');
    assert.equal(p.element('#power').getAttribute('aria-pressed'),'false','202 does not optimistically change power');
    assert.equal(p.element('#power').getAttribute('aria-busy'),'true','The loading ring follows the pending operation');
    assert.equal(p.element('.source[aria-busy=true]'),null,'Power changes do not load the source selector');
    assert.equal(p.element('.power-panel').classList.contains('is-blooming'),false,'Pending power-on does not illuminate the card');
    assert.equal(p.element('.power-control'),null,'No second progress caption');
    assert.equal(p.element('#status'),null,'No duplicate steady-state or progress sentence');
    assert.equal(p.element('#power').disabled,true);
    await p.refresh({...on,operation:{kind:'succeeded',id:operationId}});
    assert.equal(p.element('.power-heading h2').textContent,'On');
    assert.equal(p.element('#power').getAttribute('aria-busy'),'false','Loading stops once the operation completes');
    await until(()=>p.element('.power-panel').classList.contains('is-blooming'));
    assert.equal(p.element('.power-panel').classList.contains('is-on'),true,'The bloom starts on observed power-on');
    await until(()=>!p.element('.power-panel').classList.contains('is-blooming'));
    await p.refresh({...on,operation:{kind:'succeeded',id:operationId}});
    assert.equal(p.element('.power-panel').classList.contains('is-blooming'),false,'Polling does not replay the bloom');
    p.element('[data-source="wled"]').click(); await until(()=>p.requests.filter(r=>r.method==='PUT').length===2);
    assert.deepEqual(JSON.parse(p.requests.filter(r=>r.method==='PUT')[1].body),{power:true,source:'wled'});
    await delay();
    assert.equal(p.element('#power').getAttribute('aria-busy'),'false','Source changes do not spin the power button');
    assert.equal(p.element('[data-source="wled"]').getAttribute('aria-busy'),'true');
    assert.equal(p.element('[data-source="prysm"]').getAttribute('aria-busy'),'false');
    assert.equal(p.element('#power').disabled,true,'The API still serializes changes');
    await p.refresh({...wledOn,operation:{kind:'succeeded',id:operationId}});
    assert.equal(p.element('.source[aria-busy=true]'),null,'Source loading clears on completion');
    assert.equal(p.element('#power').disabled,false);
  });
  await check(on, async p => {
    p.put=()=>response({message:'Source unavailable'},503);
    p.element('[data-source="wled"]').click();
    await until(()=>p.element('.notice.error'));
    assert.equal(p.element('.source[aria-busy=true]'),null,'A rejected request clears source loading');
    assert.equal(p.element('#power').getAttribute('aria-busy'),'false');
  });
  await check(on, async p => {
    assert.equal(p.element('.power-panel').classList.contains('is-blooming'),false,'An already-on light does not bloom on page load');
    p.element('#power').click(); await until(()=>p.requests.some(r=>r.method==='PUT')); await delay();
    assert.equal(p.element('#power').getAttribute('aria-busy'),'true');
    assert.equal(p.element('.power-panel').classList.contains('is-blooming'),false,'Turning off never starts a bloom');
    await p.refresh({...off,operation:{kind:'succeeded',id:operationId}});
    assert.equal(p.element('.power-panel').classList.contains('is-blooming'),false);
  });
  await check(off, async p => {
    p.element('#power').click(); await until(()=>p.requests.some(r=>r.method==='PUT')); await delay();
    await p.refresh(off); assert.match(p.element('.power-panel').textContent,/completion is unknown/);
    p.element('#power').click(); await until(()=>p.requests.filter(r=>r.method==='PUT').length===2);
    assert.deepEqual(JSON.parse(p.requests.filter(r=>r.method==='PUT')[1].body),{power:true,source:'prysm'});
  });
  await check(offline, async p => {
    assert.equal(p.element('#power').disabled,false,'The main power button keeps the off fallback available');
    assert.equal(p.element('#power').getAttribute('aria-label'),'Turn lights off');
    assert.equal(p.element('#power').hasAttribute('aria-pressed'),false,'Unknown power must not be announced as off');
    assert.match(p.element('#status').textContent,/off command/);
    assert.equal(p.element('.sources').disabled,true,'Source changes require a known power state');
    assert.equal(p.element('.power-panel').querySelectorAll('button:enabled').length,1);
    assert.equal(p.element('#retry'),null);
    assert.equal(p.element('#off'),null);
    p.element('#power').click(); await until(()=>p.requests.some(r=>r.method==='PUT'));
    assert.deepEqual(JSON.parse(p.requests.find(r=>r.method==='PUT').body),{power:false,source:'prysm'});
  });
  await check({...off, observed:{...off.observed,live_override:0}}, async p => {
    assert.equal(p.element('.power-heading h2').textContent,'Off','Sync mismatch must not hide observed power');
    assert.equal(p.element('#power').getAttribute('aria-label'),'Turn lights on');
    assert.equal(p.element('#retry'),null,'Mismatch does not add competing actions');
    p.element('#power').click(); await until(()=>p.requests.some(r=>r.method==='PUT'));
    assert.deepEqual(JSON.parse(p.requests.find(r=>r.method==='PUT').body),{power:true,source:'prysm'});
  });
  await check({...on, observed:{...on.observed,realtime:false,pipeline_phase:'idle'}}, async p => {
    assert.equal(p.element('.power-heading h2').textContent,'On');
    assert.match(p.element('#status').textContent,/Select Screen sync/);
    p.element('[data-source="prysm"]').click(); await until(()=>p.requests.some(r=>r.method==='PUT'));
    assert.deepEqual(JSON.parse(p.requests.find(r=>r.method==='PUT').body),{power:true,source:'prysm'},'Reselecting the source reconnects without an extra retry button');
  });
  await check(on, async p => {
    p.failure='Network unavailable'; await p.refresh();
    assert.equal(p.element('#power').disabled,true,'A controller outage disables the off fallback');
    assert.equal(p.element('.power-panel').querySelectorAll('button:enabled').length,0);
    assert.match(p.element('#status').textContent,/Reconnecting automatically/);
  });
  await check(on, async p => {
    tune(p); await delay(); button(p,'Open preview').click(); await until(()=>p.sockets.length===1 && p.sockets[0].sent.length===1);
    const socket=p.sockets[0]; assert.equal(socket.url,'ws://localhost:8787/api/preview');
    let finishDecode; p.decode=()=>new Promise(resolve=>{finishDecode=resolve;});
    const pending=socket.message(frame); await delay();
    assert.equal(socket.sent.length,1,'Wait for browser decode before requesting again');
    assert.equal(p.draws.length,0);
    finishDecode(); await pending; p.decode=null;
    assert.match(p.element('.preview-meta').textContent,/Frame 9.*revision 1/);
    assert.deepEqual(p.draws.find(draw=>draw[0]==='crop'),['crop',12,27,480,240],'Crop coordinates scale from source pixels');
    assert.ok(p.draws.some(draw=>JSON.stringify(draw)===JSON.stringify(['fill','rgb(255,0,0)',12,282,240,12])),'Bottom colors keep source left-to-right coordinates');
    assert.ok(p.draws.some(draw=>JSON.stringify(draw)===JSON.stringify(['fill','rgb(0,255,0)',0,12,12,135])),'Left colors keep source top-to-bottom coordinates');
    await until(()=>socket.sent.length===2);
    assert.equal(p.element('[aria-label="Preview controls"]').getAttribute('role'), 'group');
    button(p,'Pause preview').click(); await delay(); assert.equal(socket.closed,true);
    assert.equal(p.element('.preview .badge').textContent,'Paused');
    assert.equal(p.element('.preview-state'),null,'Pausing does not add a status paragraph');
    assert.equal(p.element('.preview-rate'),null,'No duplicate state below the picture');
    button(p,'Resume preview').click(); await until(()=>p.sockets.length===2);
    p.hidden=true;p.document.dispatchEvent(new p.dom.window.Event('visibilitychange'));await delay();assert.equal(p.sockets[1].closed,true);
    p.hidden=false;p.document.dispatchEvent(new p.dom.window.Event('visibilitychange'));await until(()=>p.sockets.length===3);
    button(p,'Overview').click(); await delay();assert.equal(p.sockets[2].closed,true,'Navigation tears down preview transport');
    assert.ok(p.element('.preview-meta').textContent.includes('Frame 9'),'Last image metadata is retained');
  });
  await check({...on,pipeline:{...on.pipeline,phase:'running'}}, async p => {
    tune(p); await delay(); button(p,'Open preview').click(); await until(()=>p.sockets.length===1);
    await p.sockets[0].message({type:'busy'}); await delay();assert.equal(p.element('.preview .badge').textContent,'Waiting');assert.equal(p.element('.preview-state'),null,'Automatic retries do not add status paragraphs');
    await until(()=>p.sockets[0].sent.length===2);
    await p.sockets[0].message({type:'frame',jpeg:'invalid'});await delay();assert.equal(p.sockets[0].closed,true);assert.ok(button(p,'Retry preview'));
    button(p,'Close preview').click();await delay();assert.equal(p.element('.preview .badge').textContent,'Closed');
    assert.equal(p.requests.filter(r=>r.method==='PUT').length,0,'Preview failure never changes lighting');
  });
  await check(on, async p => {
    tune(p); await delay(); button(p,'Open preview').click(); await until(()=>p.sockets.length===1 && p.sockets[0].sent.length===1);
    await p.sockets[0].message({...frame,width:65535,height:65535}); await delay();
    assert.equal(p.sockets[0].closed,true,'Reject oversized canvas allocation');
    assert.equal(p.draws.length,0);
    button(p,'Retry preview').click();await until(()=>p.sockets.length===2 && p.sockets[1].sent.length===1);
    await p.sockets[1].message({...frame,viewport:{x:0,y:350,width:640,height:20}});await delay();
    assert.equal(p.sockets[1].closed,true,'Reject crop outside the paired source');
    button(p,'Retry preview').click();await until(()=>p.sockets.length===3 && p.sockets[2].sent.length===1);
    let finishDecode;p.decode=()=>new Promise(resolve=>{finishDecode=resolve;});
    const pending=p.sockets[2].message(frame);await delay();p.previewTimeout();await delay();
    assert.equal(p.sockets[2].closed,true,'A stalled decode has a bounded lifetime');
    assert.match(p.element('.preview-state').textContent,/timed out/);
    finishDecode();await pending;assert.equal(p.draws.length,0,'A timed-out decode cannot draw an obsolete response');
  });
  await check(on, async p => {
    let reads=0;p.configGet=async()=>response(++reads===1?config():config(45,'2'));
    await p.refresh({...on,config_revision:'2',applied_revision:'2'});assert.equal(reads,2);
    tune(p);await delay();assert.equal(p.element('#shadow_lift_percent').value,'45');
    p.hidden=true;const count=p.requests.length;await p.refresh();assert.equal(p.requests.length,count);
  });
  console.log('Control page checks passed: six-setting transaction, validation, drafts, conflicts, application status, power/source recovery, preview decode/backpressure/teardown and coherent polling.');
})().catch(error=>{console.error(error);process.exitCode=1;});
