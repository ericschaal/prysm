// Run after npm ci in prysm/control-ui: node prysm/tests/control-page.test.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('../control-ui/node_modules/jsdom');
const html = fs.readFileSync(path.join(__dirname, '../src/control.html'), 'utf8');
const off = { source: 'prysm', observed: { power: false, realtime: false, live_override: 2,
  service: 'inactive', substate: 'dead' }, pending: false, operation_id: 0,
  observation_error: null, error: null, wled_url: 'http://192.168.18.39/' };
const on = { ...off, observed: { ...off.observed, power: true, realtime: true,
  live_override: 0, service: 'active', substate: 'running' } };
const offline = { ...off, observed: null, observation_error: 'WLED offline' };
const delay = () => new Promise(resolve => setTimeout(resolve, 5));
async function until(check) {
  for (let i = 0; i < 400; i++) {
    if (check()) return;
    await delay();
  }
  assert.fail('Timed out waiting for UI');
}

function page(initial = off) {
  const state = { data: initial, requests: [], failure: null, hidden: false, put: null, get: null };
  const dom = new JSDOM(html, {
    url: 'http://localhost:8787/', runScripts: 'dangerously', pretendToBeVisual: true,
    beforeParse(window) {
      window.AbortController = AbortController;
      Object.defineProperty(window.document, 'hidden', { get: () => state.hidden });
      window.setInterval = (callback, ms) => { assert.equal(ms, 2000); state.poll = callback; return 1; };
      window.clearInterval = () => {};
      window.fetch = async (url, options) => {
        assert.equal(url, '/api/state');
        state.requests.push(options);
        if (options.method === 'PUT') {
          if (state.put) return state.put(options);
          state.data = { ...state.data, pending: true, operation_id: state.data.operation_id + 1 };
          const operation_id = state.data.operation_id;
          return { ok: true, json: async () => ({ operation_id }) };
        }
        if (state.get) return state.get();
        if (state.failure) throw Error(state.failure);
        return { ok: true, json: async () => state.data };
      };
    },
  });
  state.dom = dom;
  state.document = dom.window.document;
  state.element = selector => state.document.querySelector(selector);
  state.refresh = async data => {
    if (data) state.data = data;
    await state.poll();
    await delay();
  };
  state.ready = () => until(() => state.element('#status') && !state.element('#status').textContent.includes('Connecting'));
  return state;
}

async function check(initial, test) {
  const state = page(initial);
  try { await state.ready(); await test(state); }
  finally { await delay(); state.dom.window.close(); }
}

(async () => {
  await check(off, async p => {
    assert.match(p.element('h2').textContent, /Lights are off/);
    assert.equal(p.element('.power-panel').classList.contains('is-on'), false);
    assert.match(p.element('#power').textContent, /Turn lights on/);
    assert.equal(p.element('#off'), null, 'Normal controls have a single power action');
    assert.equal(p.element('#wled').href, off.wled_url);
    assert.match(p.element('#wled').rel, /noopener/);
    p.element('[data-source="wled"]').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    const request = p.requests.find(r => r.method === 'PUT');
    assert.deepEqual(JSON.parse(request.body), { power: false, source: 'wled' });
    assert.equal(request.headers.Authorization, undefined);
    await until(() => p.element('#power').disabled);
    await p.refresh({ ...off, source: 'wled', operation_id: 1 });
    await until(() => p.element('[data-source="wled"]').getAttribute('aria-pressed') === 'true');
    assert.equal(p.element('#power').disabled, false);
    assert.match(p.element('h2').textContent, /off/);
  });

  await check(on, async p => {
    assert.match(p.element('#status').textContent, /Following the colors/);
    assert.match(p.element('#power').textContent, /Turn lights off/);
    assert.equal(p.element('.power-panel').classList.contains('is-on'), true);
    p.element('#power').click();
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    assert.equal(p.requests.filter(r => r.method === 'PUT').length, 1, 'Double clicks submit one change');
    assert.deepEqual(JSON.parse(p.requests.find(r => r.method === 'PUT').body), { power: false, source: 'prysm' });
    await until(() => p.element('#power').disabled);
    assert.equal(p.element('[data-source="wled"]').matches(':disabled'), true);
    // An old GET must not unlock controls before the accepted operation is observed.
    await p.refresh(on);
    assert.equal(p.element('#power').disabled, true);
    await p.refresh({ ...off, operation_id: 1 });
    await until(() => !p.element('#power').disabled);
    assert.equal(p.element('.power-panel').classList.contains('is-on'), false, 'Card darkens when Off is confirmed');
    assert.match(p.element('#power').textContent, /Turn lights on/);
  });

  await check(offline, async p => {
    assert.match(p.element('#status').textContent, /unknown/i);
    assert.equal(p.element('#power').disabled, true);
    assert.equal(p.element('[data-source="prysm"]').matches(':disabled'), true);
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
    await p.refresh({ ...on, observed: { ...on.observed, substate: 'auto-restart' } });
    assert.match(p.element('#status').textContent, /restarting/);
  });

  await check(on, async p => {
    p.failure = 'Network unavailable';
    await p.refresh();
    assert.match(p.element('h2').textContent, /unknown/i);
    assert.match(p.element('#status').textContent, /Connection lost/);
    assert.equal(p.element('#power').disabled, true);
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

  await check(off, async p => {
    p.put = async () => { throw Error('Connection interrupted'); };
    p.element('[data-source="wled"]').click();
    await until(() => p.element('#retry'));
    assert.match(p.element('#status').textContent, /not confirmed/);
    p.put = null;
    p.element('#retry').click();
    await until(() => p.requests.filter(r => r.method === 'PUT').length === 2);
    assert.deepEqual(JSON.parse(p.requests.filter(r => r.method === 'PUT')[1].body), { power: false, source: 'wled' });
    await until(() => p.element('#power').disabled);
    await p.refresh({ ...off, source: 'wled', operation_id: 1 });
    await until(() => !p.element('#power').disabled);
    // Completed requests must not remain the retry target for later external changes.
    await p.refresh({ ...on, source: 'wled', operation_id: 1 });
    p.element('#retry').click();
    await until(() => p.requests.filter(r => r.method === 'PUT').length === 3);
    assert.deepEqual(JSON.parse(p.requests.filter(r => r.method === 'PUT')[2].body), { power: true, source: 'wled' });
  });

  await check(off, async p => {
    let resolveGet;
    p.get = () => new Promise(resolve => { resolveGet = resolve; });
    const stalePoll = p.poll();
    p.element('#power').click();
    await until(() => p.requests.some(r => r.method === 'PUT'));
    resolveGet({ ok: true, json: async () => ({ ...off, source: 'wled' }) });
    await stalePoll;
    await delay();
    assert.equal(p.element('[data-source="prysm"]').getAttribute('aria-pressed'), 'true', 'Pre-change GET cannot overwrite the newer request');
    assert.equal(p.element('#power').disabled, true);
    p.get = null;
    await p.refresh({ ...on, operation_id: 1 });
    await until(() => !p.element('#power').disabled);
  });
  console.log('Control page checks passed: power, sources, pending, recovery, polling, and stale responses.');
})().catch(error => { console.error(error); process.exitCode = 1; });
