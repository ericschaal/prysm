const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('../control-ui/node_modules/jsdom');
const dist = path.join(__dirname, '../control-ui/dist');
const html = fs.readFileSync(path.join(dist, 'index.html'), 'utf8');
const script = fs.readFileSync(
  path.join(dist, html.match(/<script[^>]+src="([^"]+)"/)[1]),
  'utf8',
);
async function until(check, timeout = 1000) {
  for (let i = 0; i < timeout / 5; i++) {
    if (check()) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  assert.fail('UI did not settle');
}
async function fixture(
  t,
  loadingCheck,
  { storedToken, joinErrors = [], waitForJoin = true } = {},
) {
  const calls = [];
  let failSave = false;
  let failUpdate = false;
  let view = {
    token: 'phone-token',
    pairing: null,
    connected: true,
    draft: {
      id: 'profile',
      name: 'Calibration 1',
      correction: { warmth: 0, tint: 0, midtones: 0 },
    },
    step: 'choose',
    shade: 0,
    paused: false,
    compare: false,
    sequence: 0,
    ended: null,
  };
  const settings = {
    revision: '1',
    missing_required: [],
    settings: {
      source: 'prysm',
      input: {
        type: 'webos',
        capture_resolution: [640, 360],
        refresh_rate_hz: 30,
      },
      output: {
        type: 'wled',
        url: 'http://wled/',
        leds: { top: 1, right: 1, bottom: 1, left: 1 },
      },
      processing: {
        brightness_percent: 100,
        shadow_lift_percent: 0,
        smoothing_seconds: 0.1,
        sample_density: 128,
        spatial_reach_percent: 10,
        remove_black_bars: true,
      },
      calibration: { active: null, profiles: [] },
    },
  };
  const dom = new JSDOM(html, {
    url: 'http://localhost:8787/#calibrate=pairing',
    runScripts: 'outside-only',
    pretendToBeVisual: true,
  });
  t.after(() => dom.window.close());
  const w = dom.window;
  if (storedToken)
    w.sessionStorage.setItem('prysm-calibration-pairing', storedToken);
  for (const [name, value] of Object.entries({
    Request,
    Response,
    Headers,
    AbortController,
    AbortSignal,
  }))
    w[name] = value;
  w.HTMLElement.prototype.setPointerCapture = () => {};
  w.fetch = async (request) => {
    const route = new URL(request.url).pathname;
    let result;
    let status = 200;
    if (route === '/api/settings') {
      if (loadingCheck) await new Promise((r) => setTimeout(r, 150));
      result = settings;
    } else if (route === '/api/state') {
      if (request.method === 'PUT') {
        calls.push(JSON.parse(await request.text()));
        view = { ...view, ended: 'Lighting stopped.' };
        result = { operation_id: 'stop' };
      } else
        result = {
          config_revision: '1',
          source: 'prysm',
          calibration_active: true,
          observed: null,
          operation: { kind: 'idle' },
          pipeline: { phase: 'running', frames_processed: 1 },
          wled_url: 'http://wled/',
          applied_revision: '1',
        };
    } else {
      assert.equal(route, '/api/calibration');
      const body = JSON.parse(await request.text());
      calls.push(body);
      if (['pair', 'poll'].includes(body.action) && joinErrors.length) {
        const failure = joinErrors.shift();
        if (failure === 'network') throw TypeError('Network unavailable');
        return new Response(
          JSON.stringify({
            message:
              failure === 409
                ? 'Session expired; start again on the TV'
                : 'Receiver unavailable',
          }),
          {
            status: failure,
            headers: { 'Content-Type': 'application/json' },
          },
        );
      }
      if (body.action === 'update' && failUpdate) {
        failUpdate = false;
        throw Error('Adjustment request dropped');
      }
      if (body.action === 'select')
        view = {
          ...view,
          step: 'response',
          sequence: view.sequence + 1,
          draft: {
            ...view.draft,
            correction: { warmth: 0, tint: 0, midtones: 0 },
          },
        };
      if (body.action === 'update' && body.sequence > view.sequence)
        view = {
          ...view,
          ...body,
          draft: { ...view.draft, correction: body.correction },
        };
      if (body.action === 'save') {
        if (failSave) {
          result = { message: 'Disk is full' };
          status = 500;
        } else view = { ...view, ended: 'Calibration saved and active.' };
      }
      if (body.action === 'cancel')
        view = { ...view, ended: 'Calibration ended on the TV.' };
      result ??= view;
    }
    return new Response(JSON.stringify(result), {
      status,
      headers: { 'Content-Type': 'application/json' },
    });
  };
  w.eval(script);
  if (loadingCheck) {
    await until(() => w.document.querySelector('h1'));
    loadingCheck(w.document);
  }
  const button = (name) =>
    [...w.document.querySelectorAll('button')].find(
      (b) => b.textContent === name,
    );
  if (waitForJoin) await until(() => button('New profile'), 3000);
  async function click(name) {
    const b = button(name);
    assert.ok(b, name);
    assert.equal(b.disabled, false, name);
    b.click();
    await new Promise((r) => setTimeout(r, 10));
  }
  return {
    w,
    calls,
    button,
    click,
    view: () => view,
    failUpdate: () => {
      failUpdate = true;
    },
    failSave: () => {
      failSave = true;
    },
    allowSave: () => {
      failSave = false;
    },
  };
}
for (const [label, storedToken, failure] of [
  ['pairing after a network failure', undefined, 'network'],
  ['rejoining after a network failure', 'phone-token', 'network'],
  ['pairing after a server failure', undefined, 503],
]) {
  test(`retries ${label} without reloading`, async (t) => {
    const f = await fixture(t, undefined, {
      storedToken,
      joinErrors: [failure],
    });
    const attempts = f.calls.filter(
      (c) => c.action === (storedToken ? 'poll' : 'pair'),
    );
    assert.equal(attempts.length, 2);
    assert.deepEqual(attempts[1], attempts[0]);
    assert.equal(f.w.document.querySelector('[role=alert]'), null);
    assert.equal(
      f.w.sessionStorage.getItem('prysm-calibration-pairing'),
      'phone-token',
    );
  });
}
for (const storedToken of [undefined, 'expired-phone-token']) {
  test(`stops retrying an expired ${storedToken ? 'rejoin' : 'pairing'} capability`, async (t) => {
    const f = await fixture(t, undefined, {
      storedToken,
      joinErrors: [409],
      waitForJoin: false,
    });
    await until(() => f.w.document.querySelector('[role=alert]'));
    await new Promise((resolve) => setTimeout(resolve, 1200));
    assert.equal(f.calls.length, 1);
    assert.match(
      f.w.document.querySelector('[role=alert]').textContent,
      /expired/,
    );
    assert.equal(f.w.sessionStorage.length, 0);
    assert.equal(f.button('New profile'), undefined);
  });
}
test('leaving calibration cancels a scheduled initial connection retry', async (t) => {
  const f = await fixture(t, undefined, {
    joinErrors: ['network'],
    waitForJoin: false,
  });
  await until(() => f.w.document.querySelector('[role=alert]'));
  f.w.location.hash = '#/overview';
  await until(() => f.w.document.querySelector('.app-shell'));
  await new Promise((resolve) => setTimeout(resolve, 1200));
  assert.equal(f.calls.length, 1);
  assert.equal(f.w.sessionStorage.length, 0);
});
test('wizard requires physical confirmation, manually advances shades, bypasses all correction, and retains a failed save', async (t) => {
  const f = await fixture(t);
  await f.click('New profile');
  assert.equal(f.button('Next').disabled, true);
  f.w.document.querySelector('input[type=checkbox]').click();
  await until(() => !f.button('Next').disabled);
  await f.click('Next');
  await until(() => f.w.document.querySelector('input[type=range]'));
  const range = f.w.document.querySelector('input[type=range]');
  Object.getOwnPropertyDescriptor(
    f.w.HTMLInputElement.prototype,
    'value',
  ).set.call(range, '40');
  range.dispatchEvent(new f.w.Event('input', { bubbles: true }));
  await until(() => f.view().draft.correction.warmth === 40);
  f.button('Hold for uncorrected').dispatchEvent(
    new f.w.Event('pointerdown', { bubbles: true }),
  );
  await until(() => f.view().compare);
  f.w.dispatchEvent(new f.w.Event('blur'));
  await until(() => !f.view().compare);
  assert.equal(f.view().draft.correction.warmth, 40);
  await f.click('Next');
  assert.equal(f.view().step, 'intensity');
  await f.click('Next');
  assert.equal(f.view().step, 'colors');
  await f.click('Cyan');
  assert.equal(f.view().shade, 3);
  assert.equal(f.view().paused, true);
  await f.click('Adjust dark tones');
  assert.equal(f.view().step, 'grays');
  await f.click('Dark');
  assert.equal(f.view().shade, 2);
  await f.click('Done');
  await f.click('Next');
  assert.equal(f.view().step, 'mixed');
  await f.click('Next');
  assert.equal(f.view().step, 'review');
  f.failSave();
  await f.click('Save and use');
  await until(() => f.w.document.body.textContent.includes('Disk is full'));
  assert.equal(f.view().ended, null);
  await f.click('Back');
  assert.equal(f.view().step, 'mixed');
  await f.click('Next');
  f.allowSave();
  await f.click('Save and use');
  await until(() =>
    f.w.document.body.textContent.includes('Calibration saved and active.'),
  );
  assert.equal(f.w.sessionStorage.length, 0);
  f.w.document.querySelector('a').click();
  await until(() => f.w.document.querySelector('.app-shell'));
});
test('a dropped adjustment is retried after polling without another edit', async (t) => {
  const f = await fixture(t);
  await f.click('New profile');
  f.w.document.querySelector('input[type=checkbox]').click();
  await until(() => !f.button('Next').disabled);
  await f.click('Next');
  f.failUpdate();
  const range = f.w.document.querySelector('input[type=range]');
  Object.getOwnPropertyDescriptor(
    f.w.HTMLInputElement.prototype,
    'value',
  ).set.call(range, '40');
  range.dispatchEvent(new f.w.Event('input', { bubbles: true }));
  await until(() => f.w.document.querySelector('[role=alert]'));
  assert.equal(f.view().draft.correction.warmth, 0);
  await new Promise((resolve) => setTimeout(resolve, 1200));
  await until(() => f.view().draft.correction.warmth === 40);
  assert.equal(range.value, '40');
  assert.equal(f.w.document.querySelector('[role=alert]'), null);
  const updates = f.calls.filter(
    (c) => c.action === 'update' && c.correction.warmth === 40,
  );
  assert.equal(updates.length, 2);
  assert.equal(updates[0].sequence, updates[1].sequence);
});
test('Stop lighting remains available without completing the response check', async (t) => {
  const f = await fixture(t);
  await f.click('Stop lighting');
  assert.ok(f.calls.some((c) => c.power === false && c.source === 'prysm'));
});

test('relay accepts webOS named file origins and rejects web pages and unrelated windows', async (t) => {
  const dom = new JSDOM('<!doctype html>', {
    url: 'http://localhost:8787/calibration-tv.html',
    runScripts: 'outside-only',
  });
  t.after(() => dom.window.close());
  const w = dom.window;
  const requests = [];
  w.fetch = async (_url, options) => {
    requests.push(JSON.parse(options.body));
    return { ok: true, json: async () => ({ token: 'tv-token' }) };
  };
  w.eval(
    fs.readFileSync(
      path.join(__dirname, '../control-ui/public/calibration-tv.js'),
      'utf8',
    ),
  );
  const send = (origin, source = w) =>
    w.dispatchEvent(
      new w.MessageEvent('message', {
        origin,
        source,
        data: { action: 'begin' },
      }),
    );
  send('http://attacker.invalid');
  send('file://io.prysm.capture-webos', {});
  await new Promise(setImmediate);
  assert.equal(requests.length, 0);
  send('file://io.prysm.capture-webos');
  await new Promise(setImmediate);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].action, 'begin');
});

test('returning to Choose and selecting a fresh profile keeps update ordering live', async (t) => {
  const f = await fixture(t);
  await f.click('New profile');
  f.w.document.querySelector('input[type=checkbox]').click();
  await until(() => !f.button('Next').disabled);
  await f.click('Next');
  await f.click('Back');
  await f.click('Back');
  assert.equal(f.view().step, 'choose');
  await f.click('New profile');
  await until(() => f.view().step === 'response');
  await f.click('Next');
  assert.equal(f.view().step, 'white_balance');
});

test('calibration deep link never flashes the dashboard while settings load', async (t) => {
  await fixture(t, (document) => {
    assert.equal(!!document.querySelector('.app-shell'), false);
    assert.match(document.querySelector('h1').textContent, /Connecting/);
  });
});

test('idle blur and pagehide do not issue redundant comparison updates', async (t) => {
  const f = await fixture(t);
  const count = f.calls.length;
  f.w.dispatchEvent(new f.w.Event('blur'));
  f.w.dispatchEvent(new f.w.Event('pagehide'));
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(f.calls.length, count);
});
