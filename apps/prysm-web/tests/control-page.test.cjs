// Run after npm ci in apps/prysm-web/control-ui: node apps/prysm-web/tests/control-page.test.cjs
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
    output: { type: 'wled', address: '192.168.18.39:4048', leds: { top: 10, right: 10, bottom: 10, left: 10 } },
    wled: { url: 'http://192.168.18.39/' }, processing: { ...processing, shadow_lift_percent: lift } };
}
const response = (data, status = 200) => new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } });
const off = { source: 'prysm', observed: { power: false, realtime: false, live_override: 2,
  pipeline_phase: 'idle' }, operation: { kind: 'idle' }, config_revision: revision, applied_revision: null, restart_required: false, pipeline: { phase: 'idle', applied_revision: null, frames_processed: 0, error: null },
  observation_error: null, wled_url: 'http://192.168.18.39/' };
const on = { ...off, observed: { ...off.observed, power: true, realtime: true,
  live_override: 0, pipeline_phase: 'running' }, applied_revision: revision };
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
  const state = { data: initial, config: initialConfig, configGets: 0, configGet: null, requests: [], configRequests: [], configPut: null, failure: null, hidden: false, put: null, get: null };
  const dom = new JSDOM(html, {
    url: 'http://localhost:8787/', runScripts: 'dangerously', pretendToBeVisual: true,
    beforeParse(window) {
      window.AbortController = AbortController;
      window.AbortSignal = AbortSignal;
      window.Request = Request;
      window.Response = Response;
      window.Headers = Headers;
      Object.defineProperty(window.document, 'hidden', { get: () => state.hidden });
      window.setInterval = (callback, ms) => { assert.equal(ms, 2000); state.poll = callback; return 1; };
      window.clearInterval = () => {};
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
          assert.equal(update.expected_revision, state.config.revision);
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

(async () => {
  await check(off, async p => {
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    await delay();
    await p.refresh(off);
    assert.equal(p.element('#power').disabled, false, 'Backend restart releases the lost operation');
    assert.match(p.element('details').textContent, /completion is unknown/i);
    assert.equal(p.element('#retry').disabled, false);
    p.element('#retry').click();
    await until(() => p.requests.filter(r => r.method === 'PUT').length === 2);
    assert.deepEqual(JSON.parse(p.requests.filter(r => r.method === 'PUT')[1].body), { power: true, source: 'prysm' });
  });

  await check({ ...off, restart_required: true }, async p => {
    assert.match(p.element('#status').textContent, /restart prysm/i);
    assert.equal(p.element('#power').disabled, true);
    assert.equal(p.element('#retry').disabled, true);
    assert.equal(p.element('#off').disabled, true);
    await p.refresh({ ...off, restart_required: false });
    assert.equal(p.element('#power').disabled, false);
    assert.doesNotMatch(p.element('#status').textContent, /restart prysm/i);
  }, { ...config(), output: { type: 'egui' }, wled: null });

  for (const directConfig of [ { ...config(), output: { type: 'egui' }, wled: null },
    { ...config(), wled: null },
  ]) {
  await check({ ...off, wled_url: '' }, async p => {
    assert.equal(p.element('#power').disabled, false);
    assert.equal(p.element('#power').getAttribute('aria-label'), 'Start output');
    assert.equal(p.element('.eyebrow').textContent, 'OUTPUT');
    assert.equal(p.element('#wled'), null);
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    assert.deepEqual(JSON.parse(p.requests.find(r => r.method === 'PUT').body), { power: true, source: 'prysm' });
    await p.refresh({ ...on, wled_url: '', operation: { kind: 'succeeded', id: operationId } });
    assert.equal(p.element('.sources'), null);
    assert.equal(p.element('#power').getAttribute('aria-label'), 'Stop output');
    await p.refresh({ ...on, wled_url: '', observed: { ...on.observed, pipeline_phase: 'completed' },
      pipeline: { ...on.pipeline, phase: 'completed' } });
    assert.equal(p.element('.connection').textContent, 'Connected');
    assert.match(p.element('#status').textContent, /Playback complete/);
    assert.equal(p.element('#retry'), null);
    p.element('#power').click();
    await until(() => p.requests.filter(r => r.method === 'PUT').length === 2);
    assert.deepEqual(JSON.parse(p.requests.filter(r => r.method === 'PUT')[1].body), { power: false, source: 'prysm' });
  }, directConfig);
  }

  await check(off, async p => {
    assert.match(p.element('h2').textContent, /^Off$/);
    assert.equal(p.element('.power-panel').classList.contains('is-on'), false);
    assert.match(p.element('#power').getAttribute('aria-label'), /Turn lights on/);
    assert.equal(p.element('#off'), null, 'Normal controls have a single power action');
    assert.equal(p.element('#status').classList.contains('visible'), false);
    assert.equal(p.element('#power').getAttribute('aria-pressed'), 'false');
    assert.equal(p.element('.sources'), null, 'Sources are hidden while Off');
    assert.equal(p.element('#wled').href, off.wled_url);
    assert.match(p.element('#wled').rel, /noopener/);
    await p.refresh({ ...off, source: 'wled' });
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    assert.deepEqual(JSON.parse(p.requests.find(r => r.method === 'PUT').body), { power: true, source: 'wled' });
    assert.equal(p.element('.sources'), null, 'Sources wait for confirmed On');
    await p.refresh({ ...wledOn, operation: { kind: 'succeeded', id: operationId } });
    await until(() => p.element('.sources'));
    assert.equal(p.element('[data-source="wled"]').getAttribute('aria-pressed'), 'true', 'Power restores the remembered source');
  });

  await check(on, async p => {
    p.element('[data-source="prysm"]').click();
    await delay();
    assert.equal(p.requests.filter(r => r.method === 'PUT').length, 0, 'The selected source does not resubmit a change');
    p.element('[data-source="wled"]').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    const request = p.requests.find(r => r.method === 'PUT');
    assert.deepEqual(JSON.parse(request.body), { power: true, source: 'wled' });
    assert.equal(request.headers.Authorization, undefined);
    await until(() => p.element('#power').disabled);
    assert.equal(p.element('#status').textContent, '', 'Pending changes use the power animation');
    assert.equal(p.element('#status').classList.contains('visible'), false);
    assert.equal(p.element('.power-panel').getAttribute('aria-busy'), 'false', 'Source changes do not animate power');
    assert.equal(p.element('.sources').getAttribute('aria-busy'), 'true');
    assert.equal(p.element('[data-source="wled"]').getAttribute('aria-busy'), 'true');
    assert.equal(p.element('[data-source="prysm"]').getAttribute('aria-busy'), 'false');
    await p.refresh({ ...wledOn, operation: { kind: 'succeeded', id: operationId } });
    await until(() => p.element('[data-source="wled"]').getAttribute('aria-pressed') === 'true');
    assert.equal(p.element('#power').disabled, false);
    assert.equal(p.element('[data-source="wled"]').getAttribute('aria-busy'), 'false');
    assert.match(p.element('h2').textContent, /^On$/);
  });

  await check(on, async p => {
    assert.equal(p.element('#status').textContent, '', 'Healthy state has no redundant status message');
    assert.equal(p.element('#power').getAttribute('aria-pressed'), 'true');
    assert.match(p.element('#power').getAttribute('aria-label'), /Turn lights off/);
    assert.equal(p.element('.power-panel').classList.contains('is-on'), true);
    p.element('#power').click();
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    assert.equal(p.requests.filter(r => r.method === 'PUT').length, 1, 'Double clicks submit one change');
    assert.deepEqual(JSON.parse(p.requests.find(r => r.method === 'PUT').body), { power: false, source: 'prysm' });
    await until(() => p.element('#power').disabled);
    assert.equal(p.element('#status').textContent, '', 'Power transitions add no status copy');
    assert.equal(p.element('.power-panel').getAttribute('aria-busy'), 'true');
    assert.equal(p.element('[data-source="wled"]').getAttribute('aria-busy'), 'false', 'Power changes do not animate sources');
    assert.equal(p.element('[data-source="wled"]').matches(':disabled'), true);
    // An operation still running must keep controls locked.
    await p.refresh({ ...on, operation: { kind: 'pending', id: operationId } });
    assert.equal(p.element('#power').disabled, true);
    await p.refresh({ ...off, operation: { kind: 'succeeded', id: operationId } });
    await until(() => !p.element('#power').disabled);
    assert.equal(p.element('.power-panel').getAttribute('aria-busy'), 'false');
    assert.equal(p.element('.power-panel').classList.contains('is-on'), false, 'Card darkens when Off is confirmed');
    assert.equal(p.element('.sources'), null, 'Confirmed Off removes the source controls');
    assert.match(p.element('#power').getAttribute('aria-label'), /Turn lights on/);
  });

  await check({ ...on, operation: { kind: 'failed', id: operationId, error: { code: 'backend', message: 'Capture device unavailable' } } }, async p => {
    assert.match(p.element('#status').textContent, /Couldn’t apply/);
    assert.match(p.element('details').textContent, /Capture device unavailable/);
    assert.equal(p.element('#retry').disabled, false);
  });

  await check(offline, async p => {
    assert.match(p.element('#status').textContent, /unknown/i);
    assert.equal(p.element('#power').disabled, true);
    assert.equal(p.element('.sources'), null, 'Sources are hidden when status is unknown');
    assert.equal(p.element('#off').disabled, false, 'Off works without a WLED observation');
    assert.equal(p.element('#retry').textContent, 'Check connection');
    p.element('#retry').click();
    await delay();
    assert.equal(p.requests.filter(r => r.method === 'PUT').length, 0, 'Checking unknown status never changes power');
    p.element('#off').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    assert.deepEqual(JSON.parse(p.requests.find(r => r.method === 'PUT').body), { power: false, source: 'prysm' });
  });

  await check({ ...on, observed: { ...on.observed, power: false } }, async p => {
    assert.match(p.element('h2').textContent, /unknown/i, 'Active DDP with power=false is not confirmed dark');
    assert.match(p.element('#status').textContent, /not confirmed/);
    assert.ok(p.element('#off'));
    await p.refresh({ ...on, source: 'wled' });
    assert.match(p.element('#status').textContent, /not confirmed/);
    await p.refresh({ ...on, pipeline: { ...off.pipeline, phase: 'starting' }, observed: { ...on.observed, pipeline_phase: 'starting' } });
    assert.match(p.element('#status').textContent, /starting/);
  });

  await check(on, async p => {
    p.failure = 'Network unavailable';
    await p.refresh();
    assert.match(p.element('h2').textContent, /unknown/i);
    assert.match(p.element('#status').textContent, /Connection lost/);
    assert.equal(p.element('#power').disabled, true);
    assert.equal(p.element('.sources'), null, 'Connection loss hides the source controls');
    assert.equal(p.element('#off'), null, 'No unsafe action when the controller is unreachable');
    assert.equal(p.element('#retry').disabled, false);
    p.failure = null;
    await p.refresh();
    await until(() => !p.element('#power').disabled);
    assert.equal(p.element('#retry'), null);
    p.hidden = true;
    const count = p.requests.length;
    await p.refresh();
    assert.equal(p.requests.length, count, 'Hidden pages do not poll');
    p.hidden = false;
    p.document.dispatchEvent(new p.dom.window.Event('visibilitychange'));
    await until(() => p.requests.length > count);
  });

  await check(on, async p => {
    p.put = async () => { throw Error('Connection interrupted'); };
    p.element('[data-source="wled"]').click();
    await until(() => p.element('#retry'));
    assert.match(p.element('#status').textContent, /not confirmed/);
    p.put = null;
    p.element('#retry').click();
    await until(() => p.requests.filter(r => r.method === 'PUT').length === 2);
    assert.deepEqual(JSON.parse(p.requests.filter(r => r.method === 'PUT')[1].body), { power: true, source: 'wled' });
    await until(() => p.element('#power').disabled);
    await p.refresh({ ...wledOn, operation: { kind: 'succeeded', id: operationId } });
    await until(() => !p.element('#power').disabled);
    // Completed requests must not remain the retry target for later external changes.
    await p.refresh({ ...wledOn, source: 'prysm', operation: { kind: 'succeeded', id: operationId } });
    p.element('#retry').click();
    await until(() => p.requests.filter(r => r.method === 'PUT').length === 3);
    assert.deepEqual(JSON.parse(p.requests.filter(r => r.method === 'PUT')[2].body), { power: true, source: 'prysm' });
  });

  await check(on, async p => {
    let resolveGet;
    p.get = () => new Promise(resolve => { resolveGet = resolve; });
    const stalePoll = p.poll();
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    resolveGet(response({ ...on, source: 'wled' }));
    await stalePoll;
    await delay();
    assert.equal(p.element('[data-source="prysm"]').getAttribute('aria-pressed'), 'true', 'Pre-change GET cannot overwrite the newer request');
    assert.equal(p.element('#power').disabled, true);
    p.get = null;
    await p.refresh({ ...off, operation: { kind: 'succeeded', id: operationId } });
    await until(() => !p.element('#power').disabled);
  });
  await check(on, async p => {
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    await until(() => p.element('#power').disabled);
    await p.refresh({ ...off, operation: { kind: 'succeeded', id: '00000000-0000-4000-8000-000000000003' } });
    await until(() => !p.element('#power').disabled);
    assert.equal(p.element('.power-panel').classList.contains('is-on'), false, 'A later completed operation from another client unlocks controls');
  });

  function changeLift(p, value) {
    const input = p.element('#shadow-lift');
    Object.getOwnPropertyDescriptor(p.dom.window.HTMLInputElement.prototype, 'value').set.call(input, String(value));
    input.dispatchEvent(new p.dom.window.Event('input', { bubbles: true }));
  }

  await check(on, async p => {
    assert.equal(p.element('#shadow-lift').value, '0');
    assert.match(p.element('#shadow-status').textContent, /Applied/);
    changeLift(p, 20);
    changeLift(p, 70);
    await until(() => p.configRequests.length === 1);
    await until(() => /waiting/.test(p.element('#shadow-status').textContent));
    assert.deepEqual(p.configRequests, [{ ...config(70), revision: '1' }], 'Rapid changes are debounced');
    assert.match(p.element('#shadow-status').textContent, /waiting/);
    await p.refresh({ ...p.data, applied_revision: p.config.revision });
    assert.match(p.element('#shadow-status').textContent, /Applied/);
    p.configPut = async () => response({ code: 'save_failed', message: 'Save failed' }, 500);
    changeLift(p, 85);
    await until(() => /Save not confirmed/.test(p.element('#shadow-status').textContent));
    assert.equal(p.element('#shadow-lift').value, '85', 'Failed save retains the requested value');
    p.configPut = null;
    p.element('.setting-status button').click();
    await until(() => p.configRequests.length === 3);
    await until(() => /waiting/.test(p.element('#shadow-status').textContent));
    assert.deepEqual(p.configRequests[2], { ...config(85), revision: '2' });
  });

  await check(off, async p => {
    assert.equal(p.element('#shadow-lift').value, '65', 'Reload uses the saved setting');
    assert.match(p.element('#shadow-status').textContent, /applies when screen sync starts/);
    assert.equal(p.configRequests.length, 0, 'Loading a saved value must not write it back');
    p.failure = 'Offline';
    await p.refresh();
    assert.equal(p.element('#shadow-lift').disabled, true);
  }, config(65));
  await check(on, async p => {
    p.configPut = async () => {
      p.config = { ...p.config, revision: '2', processing: { ...processing, brightness_percent: 42 } };
      p.data = { ...p.data, config_revision: '2' };
      return response({ code: 'conflict', message: 'Settings changed; refresh before saving.' }, 409);
    };
    changeLift(p, 35);
    await until(() => /Save not confirmed/.test(p.element('#shadow-status').textContent));
    assert.equal(p.element('#shadow-lift').value, '35', 'A stale revision retains the draft');
    await p.refresh();
    p.configPut = null;
    p.element('.setting-status button').click();
    await until(() => p.configRequests.length === 2);
    assert.deepEqual(p.configRequests[1], { ...config(35, '2'), processing: { ...processing, brightness_percent: 42, shadow_lift_percent: 35 } }, 'Retry uses refreshed settings and preserves another client’s changes');
  });

  await check({ ...on, applied_revision: '0' }, async p => {
    assert.match(p.element('#shadow-status').textContent, /waiting/, 'Matching values without a matching applied revision are not acknowledged');
  });
  await check(wledOn, async p => assert.equal(p.element('#shadow-lift'), null));

  await check(on, async p => {
    let reads = 0;
    p.data = { ...on, config_revision: '2', applied_revision: '2' };
    p.configGet = async () => response(++reads === 1 ? config(0) : config(45, '2'));
    await p.refresh();
    assert.equal(reads, 2, 'A config revision changed during a poll is fetched again');
    assert.equal(p.element('#shadow-lift').value, '45', 'Only a coherent config and state pair is displayed');
    assert.equal(p.configRequests.length, 0);
    p.configGet = async () => response(config(0));
    await p.refresh();
    assert.equal(p.element('#shadow-lift').disabled, true, 'Repeated incoherent snapshots disable mutations');
    p.configGet = async () => response(config(45, '2'));
    await p.refresh();
    await until(() => !p.element('#shadow-lift').disabled);
  });

  await check(on, async p => {
    let resolveGet;
    p.get = () => new Promise(resolve => { resolveGet = resolve; });
    const stalePoll = p.poll();
    changeLift(p, 60);
    await until(() => p.configRequests.length === 1);
    await until(() => /waiting/.test(p.element('#shadow-status').textContent));
    resolveGet(response(on));
    await stalePoll;
    await delay();
    assert.equal(p.element('#shadow-lift').value, '60', 'An old poll cannot overwrite a saved slider value');
  });
  console.log('Control page checks passed: power, sources, pending, recovery, polling, shadow lift, save failures, and stale responses.');
})().catch(error => { console.error(error); process.exitCode = 1; });
