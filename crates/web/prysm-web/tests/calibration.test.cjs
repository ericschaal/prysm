const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('../control-ui/node_modules/jsdom');
const { calibrationFixture } = require('../control-ui/e2e/fixture.mjs');
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
  {
    storedToken,
    joinErrors = [],
    waitForJoin = true,
    profiles = [],
    source = 'prysm',
  } = {},
) {
  const calls = [];
  const receiver = calibrationFixture();
  receiver.settings.settings.source = source;
  receiver.settings.settings.calibration = { active: null, profiles };
  let failUpdate = false;
  const dom = new JSDOM(html, {
    url: 'http://localhost:8787/#/calibrate/pairing',
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
    if (route === '/api/settings' && loadingCheck)
      await new Promise((r) => setTimeout(r, 150));
    const body = request.method === 'GET' ? {} : await request.json();
    if (request.method !== 'GET') calls.push(body);
    if (['pair', 'poll'].includes(body.action) && joinErrors.length) {
      const failure = joinErrors.shift();
      if (failure === 'network') throw TypeError('Network unavailable');
      return Response.json(
        {
          message:
            failure === 409
              ? 'Session expired; start again on the TV'
              : 'Receiver unavailable',
        },
        { status: failure },
      );
    }
    if (body.action === 'update' && failUpdate) {
      failUpdate = false;
      throw Error('Adjustment request dropped');
    }
    const { json, status } = receiver.respond(route, request.method, body);
    return Response.json(json, { status });
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
    view: () => receiver.view,
    failUpdate: () => {
      failUpdate = true;
    },
    failSave: () => {
      receiver.failSave(true);
    },
    allowSave: () => {
      receiver.failSave(false);
    },
  };
}
for (const [label, storedToken, failure] of [
  ['pairing after a network failure', undefined, 'network'],
  ['rejoining after a network failure', 'local-phone', 'network'],
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
      'local-phone',
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
  await until(() => f.w.document.body.textContent.includes('Could not save'));
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
test('Stop in the profile chooser preserves the selected WLED source', async (t) => {
  const f = await fixture(t, undefined, { source: 'wled' });
  await f.click('Stop lighting');
  assert.deepEqual(
    f.calls.find((call) => call.power === false),
    { power: false, source: 'wled' },
  );
});

test('a rejected active session discards the wizard and still offers Stop', async (t) => {
  const joinErrors = [];
  const f = await fixture(t, undefined, { joinErrors });
  await f.click('New profile');
  joinErrors.push(409);
  await until(() => f.w.document.querySelector('[role=alert]'), 3000);
  assert.match(
    f.w.document.querySelector('h1').textContent,
    /calibration ended/i,
  );
  assert.match(f.w.document.body.textContent, /fresh link/);
  assert.equal(f.button('Next'), undefined);
  assert.equal(f.button('Cancel'), undefined);
  assert.equal(f.w.sessionStorage.length, 0);
  await f.click('Stop lighting');
  assert.ok(f.calls.some((call) => call.power === false));
});

test('a profile selection during a heartbeat runs once after the poll', async (t) => {
  const f = await fixture(t);
  const started = Promise.withResolvers();
  const release = Promise.withResolvers();
  const fetch = f.w.fetch;
  f.w.fetch = async (req) => {
    if (
      req.url.endsWith('/api/calibration') &&
      (await req.clone().json()).action === 'poll'
    ) {
      started.resolve();
      await release.promise;
    }
    return fetch(req);
  };
  await started.promise;
  await f.click('New profile');
  assert.equal(f.view().step, 'choose');
  release.resolve();
  await until(() => f.view().step === 'response');
  assert.equal(f.calls.filter((call) => call.action === 'select').length, 1);
});

test('rapid slider edits send only the newest draft after the in-flight adjustment', async (t) => {
  const f = await fixture(t);
  await f.click('New profile');
  f.w.document.querySelector('input[type=checkbox]').click();
  await f.click('Next');
  const release = Promise.withResolvers();
  const fetch = f.w.fetch;
  f.w.fetch = async (req) => {
    if (
      req.url.endsWith('/api/calibration') &&
      (await req.clone().json()).correction?.warmth === 10
    )
      await release.promise;
    return fetch(req);
  };
  const slider = f.w.document.querySelector('input[type=range]');
  const { set } = Object.getOwnPropertyDescriptor(
    f.w.HTMLInputElement.prototype,
    'value',
  );
  for (const value of ['10', '20', '30']) {
    set.call(slider, value);
    slider.dispatchEvent(new f.w.Event('input', { bubbles: true }));
  }
  release.resolve();
  await until(() => f.view().draft.correction.warmth === 30);
  assert.deepEqual(
    f.calls
      .filter((call) => call.correction?.warmth)
      .map((call) => call.correction.warmth),
    [10, 30],
  );
  assert.equal(slider.value, '30');
});

test('Cancel still runs if an in-flight adjustment fails', async (t) => {
  const f = await fixture(t);
  await f.click('New profile');
  f.w.document.querySelector('input[type=checkbox]').click();
  const fetch = f.w.fetch;
  const started = Promise.withResolvers();
  const release = Promise.withResolvers();
  f.w.fetch = async (req) => {
    if (
      req.url.endsWith('/api/calibration') &&
      (await req.clone().json()).action === 'update'
    ) {
      started.resolve();
      await release.promise;
      throw new f.w.TypeError('Update failed');
    }
    return fetch(req);
  };
  await f.click('Next');
  await started.promise;
  await f.click('Cancel');
  release.resolve();
  await until(() => f.view().ended);
  assert.equal(f.calls.filter((call) => call.action === 'cancel').length, 1);
});

for (const cancel of [false, true]) {
  test(
    cancel
      ? 'Cancel replaces a Save waiting for a Review retry'
      : 'Save runs once after a dropped Review update is retried',
    async (t) => {
      const f = await fixture(t);
      await f.click('New profile');
      f.w.document.querySelector('input[type=checkbox]').click();
      for (let i = 0; i < 4; i++) await f.click('Next');
      assert.equal(f.view().step, 'mixed');
      const fetch = f.w.fetch;
      const release = Promise.withResolvers();
      const retry = Promise.withResolvers();
      const sendSave = Promise.withResolvers();
      const saveStarted = Promise.withResolvers();
      let attempts = 0;
      f.w.fetch = async (req) => {
        if (req.url.endsWith('/api/calibration')) {
          const body = await req.clone().json();
          if (body.action === 'update' && body.step === 'review') {
            attempts++;
            if (attempts === 1) {
              await release.promise;
              throw new f.w.TypeError('Dropped Review update');
            }
            await retry.promise;
          }
          if (body.action === 'save') {
            saveStarted.resolve();
            await sendSave.promise;
          }
        }
        return fetch(req);
      };
      await f.click('Next');
      await f.click('Save and use');
      assert.equal(attempts, 1);
      release.resolve();
      await until(() => attempts === 2, 3500);
      if (cancel) await f.click('Cancel');
      retry.resolve();
      if (!cancel) {
        await saveStarted.promise;
        await until(() => f.button('Cancel').disabled);
      }
      sendSave.resolve();
      await until(() => f.view().ended);
      assert.equal(attempts, 2);
      assert.equal(
        f.calls.filter((call) => call.action === 'save').length,
        cancel ? 0 : 1,
      );
      assert.equal(
        f.calls.filter((call) => call.action === 'cancel').length,
        cancel ? 1 : 0,
      );
    },
  );
}

test('at profile capacity only existing profiles can be selected', async (t) => {
  const profiles = Array.from({ length: 32 }, (_, i) => ({
    id: String(i),
    name: 'Profile ' + i,
  }));
  const f = await fixture(t, undefined, { profiles });
  assert.ok(f.button('New profile').disabled);
  assert.match(
    f.w.document.body.textContent,
    /All 32 profile slots are in use/,
  );
  const select = f.w.document.querySelector('select');
  select.value = '0';
  select.dispatchEvent(new f.w.Event('change', { bubbles: true }));
  await until(() =>
    f.calls.some((call) => call.action === 'select' && call.profile === '0'),
  );
});

test('the completion screen reports both rejected and accepted but failed Stops', async (t) => {
  const f = await fixture(t);
  await f.click('Cancel');
  const fetch = f.w.fetch;
  let reject = true;
  f.w.fetch = async (req) => {
    if (!req.url.endsWith('/api/state')) return fetch(req);
    if (req.method === 'PUT' && reject)
      throw new f.w.TypeError('Controller unreachable');
    const response = await fetch(req);
    if (req.method === 'PUT') return response;
    const state = await response.json();
    state.operation = {
      kind: 'failed',
      id: 'local-stop',
      error: { message: 'WLED did not turn off' },
    };
    return Response.json(state);
  };
  await f.click('Stop lighting');
  await until(() =>
    f.w.document
      .querySelector('[role=alert]')
      ?.textContent.includes('Controller unreachable'),
  );
  reject = false;
  await f.click('Stop lighting');
  await until(
    () =>
      f.w.document
        .querySelector('[role=alert]')
        ?.textContent.includes('WLED did not turn off'),
    3500,
  );
});

test('a failed Stop preserves a dropped adjustment for retry', async (t) => {
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
  const fetch = f.w.fetch;
  f.w.fetch = (req) =>
    req.method === 'PUT'
      ? Promise.reject(new f.w.TypeError('Stop unavailable'))
      : fetch(req);
  await f.click('Stop lighting');
  await until(() => f.w.document.body.textContent.includes('Stop unavailable'));
  await new Promise((resolve) => setTimeout(resolve, 1200));
  await until(() => f.view().draft.correction.warmth === 40);
  assert.equal(range.value, '40');
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
  const replies = [];
  w.postMessage = (reply) => replies.push(reply);
  w.XMLHttpRequest = class {
    open() {}
    setRequestHeader() {}
    send(body) {
      requests.push({ request: this, body: JSON.parse(body) });
    }
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
        data: { action: 'poll' },
      }),
    );
  const fileOrigin = 'file://io.prysm.capture-webos';
  send('http://attacker.invalid');
  send(fileOrigin, {});
  assert.equal(requests.length, 0);
  send(fileOrigin);
  send(fileOrigin);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].request.timeout, 5000);
  requests[0].request.ontimeout();
  send(fileOrigin);
  assert.deepEqual(
    requests.map(({ body }) => body),
    [{ action: 'begin' }, { action: 'begin' }],
  );
  const begin = requests[1].request;
  begin.status = 200;
  begin.response = { token: 'tv-token' };
  begin.onload();
  send(fileOrigin);
  const poll = requests[2].request;
  assert.equal(poll.timeout, 5000);
  poll.ontimeout();
  send(fileOrigin);
  assert.equal(requests[3].body.token, 'tv-token');
  const retry = requests[3].request;
  retry.status = 503;
  retry.onload();
  send(fileOrigin);
  const rejected = requests[4].request;
  rejected.status = 409;
  rejected.response = { message: 'Start again on the TV.' };
  rejected.onload();
  assert.equal(replies.at(-1).terminal, true);
  assert.equal(replies.at(-1).message, rejected.response.message);
  send(fileOrigin);
  assert.equal(requests.length, 5);
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
  assert.equal(
    f.w.document.querySelector('input[type=checkbox]').checked,
    false,
  );
  assert.ok(f.button('Next').disabled);
  f.w.document.querySelector('input[type=checkbox]').click();
  await until(() => !f.button('Next').disabled);
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
