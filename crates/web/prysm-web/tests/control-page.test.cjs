// Run after npm ci in crates/web/prysm-web/control-ui: node crates/web/prysm-web/tests/control-page.test.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('../control-ui/node_modules/jsdom');
const dist = path.join(__dirname, '../control-ui/dist');
const html = fs.readFileSync(path.join(dist, 'index.html'), 'utf8');
const scriptPath = html.match(/<script[^>]+src="([^"]+)"/)[1];
const script = fs.readFileSync(path.join(dist, scriptPath), 'utf8');
const processing = {
  brightness_percent: 100,
  shadow_lift_percent: 0,
  smoothing_seconds: 0.1,
  sample_density: 10,
  spatial_reach_percent: 10,
  remove_black_bars: true,
};
const revision = '1';
const operationId = '00000000-0000-4000-8000-000000000002';
function config(lift = 0, revision = '1') {
  return {
    revision,
    source: 'prysm',
    web: { listen: '127.0.0.1:8787', allowed_hosts: ['localhost:8787'] },
    input: {
      type: 'device',
      device: '/dev/video0',
      refresh_rate_hz: 30,
      capture_resolution: [640, 360],
    },
    output: {
      type: 'wled',
      url: 'http://192.168.18.39/',
      leds: { top: 10, right: 10, bottom: 10, left: 10 },
    },
    processing: { ...processing, shadow_lift_percent: lift },
  };
}
function settingsView(value) {
  const { revision = '1', ...settings } = value ?? {
    source: 'prysm',
    input: null,
    output: null,
    processing: { ...processing },
  };
  return {
    revision,
    settings: { calibration: { profiles: [], active: null }, ...settings },
    missing_required: ['input', 'output'].filter((key) => !settings[key]),
  };
}
const response = (data, status = 200) =>
  new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
const off = {
  source: 'prysm',
  observed: {
    power: false,
    realtime: false,
    live_override: 2,
    pipeline_phase: 'idle',
  },
  operation: { kind: 'idle' },
  config_revision: revision,
  applied_revision: null,
  pipeline: {
    phase: 'idle',
    applied_revision: null,
    frames_processed: 0,
    error: null,
  },
  observation_error: null,
  wled_url: 'http://192.168.18.39/',
};
const on = {
  ...off,
  observed: {
    ...off.observed,
    power: true,
    realtime: true,
    live_override: 0,
    pipeline_phase: 'running',
  },
  pipeline: { ...off.pipeline, phase: 'running' },
  applied_revision: revision,
};
const wledOn = {
  ...off,
  source: 'wled',
  observed: { ...off.observed, power: true },
};
const offline = { ...off, observed: null, observation_error: 'WLED offline' };
const delay = () => new Promise((resolve) => setTimeout(resolve, 5));
async function until(check) {
  for (let i = 0; i < 400; i++) {
    if (check()) return;
    await delay();
  }
  assert.fail('Timed out waiting for UI');
}

function page(initial = off, initialConfig = config()) {
  const state = {
    data: initial,
    config: initialConfig,
    settings: settingsView(initialConfig),
    configGets: 0,
    configGet: null,
    requests: [],
    configRequests: [],
    configPut: null,
    sockets: [],
    deferPreviewOpen: false,
    draws: [],
    failure: null,
    hidden: false,
    put: null,
    get: null,
  };
  const dom = new JSDOM(html, {
    url: 'http://localhost:8787/',
    runScripts: 'dangerously',
    pretendToBeVisual: true,
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
        constructor(url) {
          this.url = url.toString();
          state.sockets.push(this);
          if (!state.deferPreviewOpen)
            window.setTimeout(() => this.onopen?.(), 0);
        }
        send(value) {
          assert.equal(value, 'next');
          this.sent.push(value);
        }
        close() {
          this.closed = true;
          this.readyState = 3;
        }
        message(value) {
          return this.onmessage?.({ data: JSON.stringify(value) });
        }
      };
      window.Image = class {
        async decode() {
          if (state.decode) await state.decode();
        }
      };
      window.HTMLCanvasElement.prototype.getContext = () => ({
        fillRect(...args) {
          state.draws.push(['fill', this.fillStyle, ...args]);
        },
        drawImage(...args) {
          state.draws.push(['image', ...args.slice(1)]);
        },
        strokeRect(...args) {
          state.draws.push(['crop', ...args]);
        },
        setLineDash() {},
      });
      Object.defineProperty(window.document, 'hidden', {
        get: () => state.hidden,
      });
      window.setInterval = (callback, ms) => {
        assert.equal(ms, 2000);
        state.poll = callback;
        return 1;
      };
      window.clearInterval = () => {};
      const schedule = window.setTimeout.bind(window);
      window.setTimeout = (callback, ms, ...args) => {
        if (ms === 10000) state.previewTimeout = callback;
        return schedule(callback, ms, ...args);
      };
      window.fetch = async (request) => {
        const url = new URL(request.url).pathname;
        const options = {
          method: request.method,
          headers: Object.fromEntries(request.headers),
          body: request.method === 'PUT' ? await request.text() : undefined,
        };
        if (url === '/api/settings') {
          if (options.method === 'GET') {
            state.configGets += 1;
            if (state.configGet) return state.configGet();
            if (state.failure) throw Error(state.failure);
            return response(state.settings);
          }
          assert.equal(options.method, 'PUT');
          const update = JSON.parse(options.body);
          if (!state.configPut)
            assert.equal(update.expected_revision, state.settings.revision);
          state.configRequests.push(update.settings);
          if (state.configPut) return state.configPut(update.settings);
          const nextRevision = String(Number(state.settings.revision) + 1);
          state.config = { ...update.settings, revision: nextRevision };
          state.settings = settingsView(state.config);
          state.data = { ...state.data, config_revision: nextRevision };
          return response(state.settings);
        }
        assert.equal(url, '/api/state');
        state.requests.push(options);
        if (options.method === 'PUT') {
          if (state.put) return state.put(options);
          state.data = {
            ...state.data,
            operation: { kind: 'pending', id: operationId },
          };
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
  state.element = (selector) => state.document.querySelector(selector);
  state.refresh = async (data, config) => {
    if (data) state.data = data;
    if (config !== undefined) {
      state.config = config;
      state.settings = settingsView(config);
    }
    await state.poll();
    await delay();
  };
  state.ready = () =>
    until(
      () =>
        state.element('.connection') &&
        !state.element('.connection').textContent.includes('Connecting'),
    );
  return state;
}

async function check(initial, test, initialConfig) {
  const state = page(initial, initialConfig);
  try {
    await state.ready();
    await test(state);
  } finally {
    await delay();
    state.dom.window.close();
  }
}

function button(p, text) {
  return [...p.document.querySelectorAll('button, .main-nav a')].find(
    (element) =>
      (element.getAttribute('aria-label') || element.textContent).trim() ===
      text,
  );
}
function tune(p) {
  button(p, 'Preview').click();
}
function settings(p) {
  button(p, 'Settings').click();
}
function change(p, selector, value) {
  const input = p.element(selector);
  const prototype =
    input.tagName === 'SELECT'
      ? p.dom.window.HTMLSelectElement.prototype
      : input.tagName === 'TEXTAREA'
        ? p.dom.window.HTMLTextAreaElement.prototype
        : p.dom.window.HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, 'value').set.call(
    input,
    String(value),
  );
  input.dispatchEvent(
    new p.dom.window.Event(input.tagName === 'SELECT' ? 'change' : 'input', {
      bubbles: true,
    }),
  );
}
const frame = {
  type: 'frame',
  frame_id: 9,
  revision: '1',
  source_width: 640,
  source_height: 360,
  width: 480,
  height: 270,
  viewport: { x: 0, y: 20, width: 640, height: 320 },
  colors: {
    top: [[255, 0, 0]],
    right: [[0, 255, 0]],
    bottom: [
      [255, 0, 0],
      [0, 0, 255],
    ],
    left: [
      [0, 255, 0],
      [255, 255, 0],
    ],
  },
  jpeg: 'test',
};
(async () => {
  for (const fps of [4, 5]) {
    const initial = config();
    initial.input.refresh_rate_hz = fps;
    await check(
      on,
      async (p) => {
        settings(p);
        await delay();
        assert.equal(!!p.element('#bars-warning'), fps < 5);
        if (fps < 5) {
          assert.match(
            p.element('#bars-warning').textContent,
            /at least 5 fps/,
          );
          p.element('#processing-remove_black_bars').click();
          await delay();
          assert.equal(p.element('#bars-warning'), null);
        }
      },
      initial,
    );
  }
  await check(
    off,
    async (p) => {
      assert.equal(
        p.document.querySelector('.main-nav'),
        null,
        'Incomplete effective settings show setup, not normal controls',
      );
      assert.equal(p.element('#power'), null);
      assert.equal(
        p.requests.length,
        0,
        'No state endpoint is requested before setup is complete',
      );
      assert.equal(p.element('.connection').textContent, 'Connected');
      assert.match(p.element('#settings-heading').textContent, /Video source/);
      change(p, '#input-width', 641);
      button(p, 'Next').click();
      await delay();
      assert.equal(
        p.element('#input-width').getAttribute('aria-invalid'),
        'true',
      );
      assert.equal(
        p.document.activeElement.id,
        'input-width',
        'Invalid setup step focuses its first invalid field',
      );
      change(p, '#input-width', 640);
      button(p, 'Next').click();
      await delay();
      assert.match(p.element('#settings-heading').textContent, /Your lights/);
      button(p, 'Next').click();
      await delay();
      assert.equal(
        p.element('#output-url').getAttribute('aria-invalid'),
        'true',
      );
      assert.equal(p.document.activeElement.id, 'output-url');
      change(p, '#output-url', 'http://192.168.1.20/');
      button(p, 'Next').click();
      await delay();
      assert.equal(
        p.element('#output-leds-top').getAttribute('aria-invalid'),
        'true',
        'Zero total LEDs cannot finish setup',
      );
      for (const edge of ['top', 'right', 'bottom', 'left'])
        change(p, '#output-leds-' + edge, 10);
      button(p, 'Next').click();
      await delay();
      assert.match(p.element('#settings-heading').textContent, /Review/);
      assert.match(
        p.element('.setup-review .screen-label').textContent,
        /40 LEDs/,
      );
      assert.match(
        p.element('.setup-review svg title').textContent,
        /10 top, 10 right, 10 bottom, 10 left/,
      );
      assert.equal(
        button(p, 'Edit video source').textContent,
        'Edit',
        'Compact edit controls keep descriptive accessible names',
      );
      assert.equal(
        p.element('.setup-review .review-address').textContent,
        'http://192.168.1.20/',
      );
      assert.equal(
        p.document.activeElement.id,
        'settings-heading',
        'Step changes focus the new heading',
      );
      assert.equal(
        p.document.querySelectorAll('.setup-steps .complete').length,
        2,
      );
      button(p, 'Edit video source').click();
      await delay();
      assert.equal(p.element('#input-width').value, '640');
      button(p, 'Next').click();
      await delay();
      button(p, 'Next').click();
      await delay();
      button(p, 'Edit lights').click();
      await delay();
      assert.equal(p.element('#output-url').value, 'http://192.168.1.20/');
      button(p, 'Next').click();
      await delay();
      p.element('.setup-advanced').open = true;
      change(p, '#processing-sample_density', 0);
      p.element('.setup-advanced').open = false;
      button(p, 'Finish setup').click();
      await delay();
      assert.equal(
        p.element('.setup-advanced').open,
        true,
        'Invalid advanced settings are revealed',
      );
      assert.equal(p.document.activeElement.id, 'processing-sample_density');
      assert.equal(p.configRequests.length, 0);
      change(p, '#processing-sample_density', processing.sample_density);
      button(p, 'Finish setup').click();
      await until(() => p.configRequests.length === 1 && p.element('#power'));
      assert.deepEqual(p.configRequests[0], {
        calibration: { profiles: [], active: null },
        source: 'prysm',
        processing: { ...processing },
        input: {
          type: 'webos',
          capture_resolution: [640, 360],
          refresh_rate_hz: 30,
        },
        output: {
          type: 'wled',
          url: 'http://192.168.1.20/',
          leds: { top: 10, right: 10, bottom: 10, left: 10 },
        },
      });
      assert.equal(
        p.requests.filter((request) => request.method === 'PUT').length,
        0,
        'Setup does not turn on lights',
      );
      assert.equal(p.sockets.length, 0);
      assert.equal(p.document.querySelectorAll('nav a').length, 3);
      await p.refresh();
      assert.doesNotMatch(
        p.element('.power-panel').textContent,
        /completion is unknown/,
        'An idle runtime after setup is expected, not an uncertain restart',
      );
    },
    null,
  );
  await check(off, async (p) => {
    settings(p);
    await delay();
    const editor = p.element('.led-editor');
    const ids = [...p.document.querySelectorAll('.led-layout [id]')].map(
      (node) => node.id,
    );
    assert.equal(
      ids.length,
      6,
      'Overview and editor each have their own SVG title and gradients',
    );
    assert.equal(
      new Set(ids).size,
      ids.length,
      'Both layouts can coexist without shared SVG IDs',
    );
    change(p, '#output-leds-top', 15);
    await delay();
    assert.match(editor.querySelector('.screen-label').textContent, /45 LEDs/);
    assert.match(
      editor.querySelector('title').textContent,
      /15 top, 10 right, 10 bottom, 10 left/,
    );
    assert.match(
      p.element('.setup-card .screen-label').textContent,
      /40 LEDs/,
      'Draft edits do not change saved overview counts',
    );
    for (const invalid of ['', '-1', '1.5']) {
      change(p, '#output-leds-left', invalid);
      await delay();
      assert.equal(
        p.element('#output-leds-left').value,
        invalid,
        'Keep incomplete count text for correction',
      );
      assert.equal(editor.querySelector('.screen-label').textContent, '— LEDs');
      assert.doesNotMatch(editor.textContent, /NaN/);
    }
    assert.equal(
      p.configRequests.length,
      0,
      'Layout preview never saves the draft',
    );
  });
  await check(wledOn, async (p) => {
    assert.equal(p.element('.source-options').dataset.selected, 'wled');
    assert.equal(p.element('#wled').textContent, 'Open WLED ↗');
    assert.equal(
      p.element('[data-source="wled"]').getAttribute('aria-pressed'),
      'true',
    );
  });
  await check(off, async (p) => {
    settings(p);
    await delay();
    change(p, '#processing-brightness_percent', 20);
    p.configPut = async (changes) => {
      p.config = {
        ...p.config,
        revision: '2',
        input: null,
        processing: { ...p.config.processing, ...changes.processing },
      };
      p.settings = settingsView(p.config);
      return response(p.settings);
    };
    button(p, 'Save & apply').click();
    await until(() => button(p, 'Next')?.disabled === false);
    assert.equal(p.element('#power'), null);
    assert.equal(
      p.element('.connection').textContent,
      'Connected',
      'An incomplete settings result returns to setup and releases pending controls',
    );
  });
  await check(on, async (p) => {
    assert.equal(p.sockets.length, 0, 'Overview does no preview work');
    assert.match(
      p.element('.led-layout svg title').textContent,
      /10 top, 10 right, 10 bottom, 10 left/,
    );
    assert.match(p.element('.screen-label').textContent, /40 LEDs/);
    assert.equal(p.document.querySelectorAll('nav a').length, 3);
    settings(p);
    await delay();
    for (const [key, value] of Object.entries({
      brightness_percent: 65,
      shadow_lift_percent: 25,
      smoothing_seconds: 0.8,
      sample_density: 60,
      spatial_reach_percent: 22,
    }))
      change(p, '#processing-' + key, value);
    p.element('#processing-remove_black_bars').click();
    await delay();
    assert.match(p.element('#settings-status').textContent, /Unsaved/);
    assert.equal(p.configRequests.length, 0, 'Editing never autosaves');
    button(p, 'Overview').click();
    await delay();
    assert.ok(p.element('.draft-dot'));
    settings(p);
    await delay();
    assert.equal(p.element('#processing-brightness_percent').value, '65');
    button(p, 'Save & apply').click();
    await until(() => p.configRequests.length === 1);
    await delay();
    assert.deepEqual(p.configRequests[0].processing, {
      brightness_percent: 65,
      shadow_lift_percent: 25,
      smoothing_seconds: 0.8,
      sample_density: 60,
      spatial_reach_percent: 22,
      remove_black_bars: false,
    });
    assert.equal(
      p.requests.filter((request) => request.method === 'PUT').length,
      0,
      'Saving processing issues no power or restart request',
    );
    await p.refresh({
      ...p.data,
      applied_revision: '2',
      pipeline: { ...p.data.pipeline, applied_revision: '2', phase: 'running' },
    });
    assert.match(
      p.element('#settings-status').textContent,
      /Applied to your lights/,
    );
    tune(p);
    await delay();
    assert.equal(
      p.element('.preview-page .settings'),
      null,
      'Preview is inspection only',
    );
  });
  await check(off, async (p) => {
    settings(p);
    await delay();
    assert.equal(p.element('#web-listen'), null);
    assert.equal(p.element('#web-allowed_hosts'), null);
    assert.equal(p.element('#input-listen'), null);
  });
  await check(off, async (p) => {
    settings(p);
    await delay();
    change(p, '#source', 'wled');
    change(p, '#input-device', '/dev/video2');
    change(p, '#input-width', 1280);
    change(p, '#input-height', 720);
    change(p, '#input-refresh_rate_hz', 24);
    change(p, '#output-url', 'https://wled.example/');
    change(p, '#output-leds-top', 51);
    button(p, 'Save & apply').click();
    await until(() => p.configRequests.length === 1);
    assert.deepEqual(p.configRequests[0], {
      calibration: { profiles: [], active: null },
      source: 'wled',
      input: {
        type: 'device',
        device: '/dev/video2',
        capture_resolution: [1280, 720],
        refresh_rate_hz: 24,
      },
      output: {
        type: 'wled',
        url: 'https://wled.example/',
        leds: { top: 51, right: 10, bottom: 10, left: 10 },
      },
      processing: { ...processing },
    });
    assert.equal(
      p.dom.window.location.origin,
      'http://localhost:8787',
      'Saving never redirects to a wildcard bind address',
    );
    await p.refresh();
  });
  for (const [kind, path, value] of [
    ['webos', null, null],
    ['file', 'path', '/videos/test.mkv'],
  ])
    await check(off, async (p) => {
      settings(p);
      await delay();
      change(p, '#input-type', kind);
      await delay();
      if (path) change(p, '#input-' + path, value);
      button(p, 'Save & apply').click();
      await until(() => p.configRequests.length === 1);
      assert.deepEqual(
        p.configRequests[0].input,
        {
          type: kind,
          ...(path ? { [path]: value } : {}),
          capture_resolution: [640, 360],
          refresh_rate_hz: 30,
        },
        'Changing input replaces the tagged group and drops stale branch keys',
      );
    });
  await check(off, async (p) => {
    settings(p);
    await delay();
    change(p, '#input-type', 'webos');
    await delay();
    change(p, '#input-type', 'device');
    await delay();
    assert.equal(p.element('#input-device').value, '/dev/video0');
    assert.equal(
      button(p, 'Save & apply').disabled,
      true,
      'Changes in an inactive input variant do not leave a dirty draft',
    );
    assert.equal(p.configRequests.length, 0);
    change(p, '#input-width', '');
    button(p, 'Save & apply').click();
    await delay();
    assert.equal(
      p.element('#input-width').value,
      '',
      'Incomplete numeric text is preserved until corrected',
    );
    assert.equal(
      p.element('#input-width').getAttribute('aria-invalid'),
      'true',
    );
    assert.equal(p.configRequests.length, 0);
  });
  await check(on, async (p) => {
    settings(p);
    await delay();
    change(p, '#processing-sample_density', 0);
    button(p, 'Save & apply').click();
    await delay();
    assert.equal(
      p.element('#processing-sample_density').getAttribute('aria-invalid'),
      'true',
    );
    assert.equal(p.configRequests.length, 0);
    change(p, '#processing-sample_density', 12);
    p.configPut = async () =>
      response(
        { code: 'invalid_request', message: 'Server rejected settings' },
        422,
      );
    button(p, 'Save & apply').click();
    await until(() =>
      /Server rejected/.test(p.element('.settings').textContent),
    );
    assert.equal(p.element('#processing-sample_density').value, '12');
    assert.match(p.element('.settings').textContent, /draft is preserved/);
    p.configPut = null;
    button(p, 'Save & apply').click();
    await until(() => p.configRequests.length === 2);
  });
  await check(on, async (p) => {
    settings(p);
    await delay();
    change(p, '#processing-shadow_lift_percent', 35);
    await delay();
    await p.refresh(
      { ...on, config_revision: '2' },
      {
        ...config(0, '2'),
        processing: { ...processing, brightness_percent: 42 },
      },
    );
    assert.equal(p.element('#processing-shadow_lift_percent').value, '35');
    assert.match(p.element('.settings').textContent, /changed elsewhere/);
    assert.equal(button(p, 'Save & apply').disabled, true);
    button(p, 'Reload saved settings & discard draft').click();
    await delay();
    assert.equal(p.element('#processing-brightness_percent').value, '42');
    assert.equal(p.element('#processing-shadow_lift_percent').value, '0');
  });
  await check(on, async (p) => {
    settings(p);
    await delay();
    change(p, '#processing-shadow_lift_percent', 35);
    p.configPut = async () =>
      response({ code: 'conflict', message: 'Refresh before saving' }, 409);
    button(p, 'Save & apply').click();
    await until(() =>
      /Refresh before saving/.test(p.element('.settings').textContent),
    );
    assert.equal(p.element('#processing-shadow_lift_percent').value, '35');
    assert.equal(
      button(p, 'Save & apply').disabled,
      true,
      '409 requires explicit reload',
    );
  });
  await check(on, async (p) => {
    settings(p);
    await delay();
    change(p, '#processing-brightness_percent', 20);
    p.failure = 'Network unavailable';
    await p.refresh();
    assert.equal(p.element('#processing-brightness_percent').value, '20');
    assert.equal(button(p, 'Save & apply').disabled, true);
    assert.equal(p.element('#power').disabled, true);
    p.failure = null;
    await p.refresh(on);
    assert.equal(button(p, 'Save & apply').disabled, false);
  });
  await check(on, async (p) => {
    settings(p);
    await delay();
    change(p, '#processing-brightness_percent', 20);
    p.configPut = async (changes) => {
      p.config = {
        ...p.config,
        revision: '2',
        processing: { ...p.config.processing, ...changes.processing },
      };
      p.settings = settingsView(p.config);
      p.data = {
        ...p.data,
        config_revision: '2',
        operation: {
          kind: 'failed',
          id: operationId,
          error: { code: 'backend', message: 'Apply failed' },
        },
      };
      return response(p.settings);
    };
    button(p, 'Save & apply').click();
    await until(() => /Apply failed/.test(p.element('.settings').textContent));
    assert.match(p.element('.settings').textContent, /Saved values may differ/);
    assert.match(
      p.element('#settings-status').textContent,
      /application failed/,
    );
  });
  await check(off, async (p) => {
    tune(p);
    await delay();
    assert.match(p.element('.preview-note').textContent, /Lights are off/);
    assert.equal(
      p.sockets.length,
      0,
      'The note does not open preview or change lighting',
    );
    assert.equal(button(p, 'Open preview').matches(':disabled'), true);
    button(p, 'Open preview').click();
    await delay();
    assert.equal(
      p.sockets.length,
      0,
      'Preview cannot be opened while the lights are off',
    );
    await p.refresh(wledOn);
    assert.match(
      p.element('.preview-note').textContent,
      /WLED effects are selected/,
    );
    assert.equal(button(p, 'Open preview').matches(':disabled'), true);
    await p.refresh({ ...on, pipeline: { ...on.pipeline, phase: 'idle' } });
    assert.match(
      p.element('.preview-note').textContent,
      /Screen sync is stopped/,
    );
    await p.refresh({ ...on, pipeline: { ...on.pipeline, phase: 'running' } });
    assert.equal(
      p.element('.preview-note'),
      null,
      'No warning while Screen sync is running',
    );
    assert.equal(p.requests.filter((r) => r.method === 'PUT').length, 0);
  });
  await check(on, async (p) => {
    tune(p);
    await delay();
    button(p, 'Open preview').click();
    await until(() => p.sockets.length === 1 && p.sockets[0].sent.length === 1);
    let finishDecode;
    p.decode = () =>
      new Promise((resolve) => {
        finishDecode = resolve;
      });
    const pending = p.sockets[0].message(frame);
    await delay();
    await p.refresh(off);
    await until(
      () =>
        p.sockets[0].closed && button(p, 'Open preview')?.matches(':disabled'),
    );
    assert.equal(
      p.sockets[0].closed,
      true,
      'Power off tears down preview transport',
    );
    assert.equal(button(p, 'Open preview').matches(':disabled'), true);
    assert.equal(p.element('.preview .badge').textContent, 'Unavailable');
    finishDecode();
    await pending;
    assert.equal(
      p.draws.length,
      0,
      'An in-flight decode cannot draw after power off',
    );
    await p.refresh(on);
    await until(() => button(p, 'Open preview')?.matches(':enabled'));
    assert.equal(button(p, 'Open preview').matches(':disabled'), false);
    assert.equal(
      p.sockets.length,
      1,
      'Power on does not reopen preview automatically',
    );
    assert.equal(p.element('.preview .badge').textContent, 'Closed');
  });
  await check(off, async (p) => {
    p.element('#power').click();
    p.element('#power').click();
    await until(() => p.requests.some((r) => r.method === 'PUT'));
    await delay();
    assert.equal(p.requests.filter((r) => r.method === 'PUT').length, 1);
    assert.equal(
      p.element('.power-heading h2').textContent,
      'Off',
      'Keep the observed state steady while waiting',
    );
    assert.equal(
      p.element('#power').getAttribute('aria-pressed'),
      'false',
      '202 does not optimistically change power',
    );
    assert.equal(
      p.element('#power').getAttribute('aria-busy'),
      'true',
      'The loading ring follows the pending operation',
    );
    assert.equal(
      p.element('.source[aria-busy=true]'),
      null,
      'Power changes do not load the source selector',
    );
    assert.equal(
      p.element('.power-panel').classList.contains('is-blooming'),
      false,
      'Pending power-on does not illuminate the card',
    );
    assert.equal(
      p.element('.power-control'),
      null,
      'No second progress caption',
    );
    assert.equal(
      p.element('#status'),
      null,
      'No duplicate steady-state or progress sentence',
    );
    assert.equal(p.element('#power').disabled, true);
    await p.refresh({
      ...on,
      operation: { kind: 'succeeded', id: operationId },
    });
    assert.equal(p.element('.power-heading h2').textContent, 'On');
    assert.equal(
      p.element('#power').getAttribute('aria-busy'),
      'false',
      'Loading stops once the operation completes',
    );
    await until(() =>
      p.element('.power-panel').classList.contains('is-blooming'),
    );
    assert.equal(
      p.element('.power-panel').classList.contains('is-on'),
      true,
      'The bloom starts on observed power-on',
    );
    await until(
      () => !p.element('.power-panel').classList.contains('is-blooming'),
    );
    await p.refresh({
      ...on,
      operation: { kind: 'succeeded', id: operationId },
    });
    assert.equal(
      p.element('.power-panel').classList.contains('is-blooming'),
      false,
      'Polling does not replay the bloom',
    );
    p.element('[data-source="wled"]').click();
    await until(
      () => p.requests.filter((r) => r.method === 'PUT').length === 2,
    );
    assert.deepEqual(
      JSON.parse(p.requests.filter((r) => r.method === 'PUT')[1].body),
      { power: true, source: 'wled' },
    );
    await delay();
    assert.equal(
      p.element('#power').getAttribute('aria-busy'),
      'false',
      'Source changes do not spin the power button',
    );
    assert.equal(
      p.element('[data-source="wled"]').getAttribute('aria-busy'),
      'true',
    );
    assert.equal(
      p.element('[data-source="prysm"]').getAttribute('aria-busy'),
      'false',
    );
    assert.equal(
      p.element('#power').disabled,
      true,
      'The API still serializes changes',
    );
    await p.refresh({
      ...wledOn,
      operation: { kind: 'succeeded', id: operationId },
    });
    assert.equal(
      p.element('.source[aria-busy=true]'),
      null,
      'Source loading clears on completion',
    );
    assert.equal(p.element('#power').disabled, false);
  });
  await check(on, async (p) => {
    p.put = () => response({ message: 'Source unavailable' }, 503);
    p.element('[data-source="wled"]').click();
    await until(() => p.element('.notice.error'));
    assert.equal(
      p.element('.source[aria-busy=true]'),
      null,
      'A rejected request clears source loading',
    );
    assert.equal(p.element('#power').getAttribute('aria-busy'), 'false');
  });
  await check(on, async (p) => {
    assert.equal(
      p.element('.power-panel').classList.contains('is-blooming'),
      false,
      'An already-on light does not bloom on page load',
    );
    p.element('#power').click();
    await until(() => p.requests.some((r) => r.method === 'PUT'));
    await delay();
    assert.equal(p.element('#power').getAttribute('aria-busy'), 'true');
    assert.equal(
      p.element('.power-panel').classList.contains('is-blooming'),
      false,
      'Turning off never starts a bloom',
    );
    await p.refresh({
      ...off,
      operation: { kind: 'succeeded', id: operationId },
    });
    assert.equal(
      p.element('.power-panel').classList.contains('is-blooming'),
      false,
    );
  });
  await check(off, async (p) => {
    p.element('#power').click();
    await until(() => p.requests.some((r) => r.method === 'PUT'));
    await delay();
    await p.refresh(off);
    assert.match(
      p.element('.power-panel').textContent,
      /completion is unknown/,
    );
    p.element('#power').click();
    await until(
      () => p.requests.filter((r) => r.method === 'PUT').length === 2,
    );
    assert.deepEqual(
      JSON.parse(p.requests.filter((r) => r.method === 'PUT')[1].body),
      { power: true, source: 'prysm' },
    );
  });
  await check(offline, async (p) => {
    assert.equal(
      p.element('#power').disabled,
      false,
      'The main power button keeps the off fallback available',
    );
    assert.equal(
      p.element('#power').getAttribute('aria-label'),
      'Turn lights off',
    );
    assert.equal(
      p.element('#power').hasAttribute('aria-pressed'),
      false,
      'Unknown power must not be announced as off',
    );
    assert.match(p.element('#status').textContent, /off command/);
    assert.equal(
      p.element('.sources').disabled,
      true,
      'Source changes require a known power state',
    );
    assert.equal(
      p.element('.power-panel').querySelectorAll('button:enabled').length,
      1,
    );
    assert.equal(p.element('#retry'), null);
    assert.equal(p.element('#off'), null);
    p.element('#power').click();
    await until(() => p.requests.some((r) => r.method === 'PUT'));
    assert.deepEqual(
      JSON.parse(p.requests.find((r) => r.method === 'PUT').body),
      { power: false, source: 'prysm' },
    );
  });
  await check(
    { ...off, observed: { ...off.observed, live_override: 0 } },
    async (p) => {
      assert.equal(
        p.element('.power-heading h2').textContent,
        'Off',
        'Sync mismatch must not hide observed power',
      );
      assert.equal(
        p.element('#power').getAttribute('aria-label'),
        'Turn lights on',
      );
      assert.equal(
        p.element('#retry'),
        null,
        'Mismatch does not add competing actions',
      );
      p.element('#power').click();
      await until(() => p.requests.some((r) => r.method === 'PUT'));
      assert.deepEqual(
        JSON.parse(p.requests.find((r) => r.method === 'PUT').body),
        { power: true, source: 'prysm' },
      );
    },
  );
  await check(
    {
      ...on,
      observed: { ...on.observed, realtime: false, pipeline_phase: 'idle' },
    },
    async (p) => {
      assert.equal(p.element('.power-heading h2').textContent, 'On');
      assert.match(p.element('#status').textContent, /Select Screen sync/);
      p.element('[data-source="prysm"]').click();
      await until(() => p.requests.some((r) => r.method === 'PUT'));
      assert.deepEqual(
        JSON.parse(p.requests.find((r) => r.method === 'PUT').body),
        { power: true, source: 'prysm' },
        'Reselecting the source reconnects without an extra retry button',
      );
    },
  );
  await check(on, async (p) => {
    p.failure = 'Network unavailable';
    await p.refresh();
    assert.equal(
      p.element('#power').disabled,
      true,
      'A controller outage disables the off fallback',
    );
    assert.equal(
      p.element('.power-panel').querySelectorAll('button:enabled').length,
      0,
    );
    assert.match(
      p.element('#status').textContent,
      /Reconnecting automatically/,
    );
  });
  await check(on, async (p) => {
    tune(p);
    await delay();
    button(p, 'Open preview').click();
    await until(() => p.sockets.length === 1 && p.sockets[0].sent.length === 1);
    const socket = p.sockets[0];
    assert.equal(socket.url, 'ws://localhost:8787/api/preview');
    let finishDecode;
    p.decode = () =>
      new Promise((resolve) => {
        finishDecode = resolve;
      });
    const pending = socket.message(frame);
    await delay();
    assert.equal(
      socket.sent.length,
      1,
      'Wait for browser decode before requesting again',
    );
    assert.equal(p.draws.length, 0);
    finishDecode();
    await pending;
    p.decode = null;
    assert.match(p.element('.preview-meta').textContent, /Frame 9.*revision 1/);
    assert.deepEqual(
      p.draws.find((draw) => draw[0] === 'crop'),
      ['crop', 12, 27, 480, 240],
      'Crop coordinates scale from source pixels',
    );
    assert.ok(
      p.draws.some(
        (draw) =>
          JSON.stringify(draw) ===
          JSON.stringify(['fill', 'rgb(255,0,0)', 12, 282, 240, 12]),
      ),
      'Bottom colors keep source left-to-right coordinates',
    );
    assert.ok(
      p.draws.some(
        (draw) =>
          JSON.stringify(draw) ===
          JSON.stringify(['fill', 'rgb(0,255,0)', 0, 12, 12, 135]),
      ),
      'Left colors keep source top-to-bottom coordinates',
    );
    await until(() => socket.sent.length === 2);
    assert.equal(
      p.element('[aria-label="Preview controls"]').getAttribute('role'),
      'group',
    );
    button(p, 'Pause preview').click();
    await delay();
    assert.equal(socket.closed, true);
    assert.equal(p.element('.preview .badge').textContent, 'Paused');
    assert.equal(
      p.element('.preview-state'),
      null,
      'Pausing does not add a status paragraph',
    );
    assert.equal(
      p.element('.preview-rate'),
      null,
      'No duplicate state below the picture',
    );
    p.deferPreviewOpen = true;
    button(p, 'Resume preview').click();
    await until(() => p.sockets.length === 2);
    assert.equal(p.sockets[1].sent.length, 0);
    assert.equal(
      p.element('.preview .badge').textContent,
      'Waiting',
      'The old frame is not live while the replacement connection is opening',
    );
    p.sockets[1].onopen();
    await p.sockets[1].message(frame);
    await until(() => p.element('.preview .badge').textContent === 'Live');
    p.deferPreviewOpen = false;
    p.hidden = true;
    p.document.dispatchEvent(new p.dom.window.Event('visibilitychange'));
    await delay();
    assert.equal(p.sockets[1].closed, true);
    p.hidden = false;
    p.document.dispatchEvent(new p.dom.window.Event('visibilitychange'));
    await until(() => p.sockets.length === 3);
    button(p, 'Overview').click();
    await until(() => p.sockets[2].closed);
    assert.equal(
      p.sockets[2].closed,
      true,
      'Navigation tears down preview transport',
    );
    assert.ok(
      p.element('.preview-meta').textContent.includes('Frame 9'),
      'Last image metadata is retained',
    );
  });
  await check(
    { ...on, pipeline: { ...on.pipeline, phase: 'running' } },
    async (p) => {
      tune(p);
      await delay();
      button(p, 'Open preview').click();
      await until(() => p.sockets.length === 1);
      await p.sockets[0].message({ type: 'busy' });
      await delay();
      assert.equal(p.element('.preview .badge').textContent, 'Waiting');
      assert.equal(
        p.element('.preview-state'),
        null,
        'Automatic retries do not add status paragraphs',
      );
      await until(() => p.sockets[0].sent.length === 2);
      await p.sockets[0].message({ type: 'frame', jpeg: 'invalid' });
      await delay();
      assert.equal(p.sockets[0].closed, true);
      assert.ok(button(p, 'Retry preview'));
      button(p, 'Close preview').click();
      await delay();
      assert.equal(p.element('.preview .badge').textContent, 'Closed');
      assert.equal(
        p.requests.filter((r) => r.method === 'PUT').length,
        0,
        'Preview failure never changes lighting',
      );
    },
  );
  await check(on, async (p) => {
    tune(p);
    await delay();
    button(p, 'Open preview').click();
    await until(() => p.sockets.length === 1 && p.sockets[0].sent.length === 1);
    await p.sockets[0].message({ ...frame, width: 65535, height: 65535 });
    await delay();
    assert.equal(
      p.sockets[0].closed,
      true,
      'Reject oversized canvas allocation',
    );
    assert.equal(p.draws.length, 0);
    button(p, 'Retry preview').click();
    await until(() => p.sockets.length === 2 && p.sockets[1].sent.length === 1);
    await p.sockets[1].message({
      ...frame,
      viewport: { x: 0, y: 350, width: 640, height: 20 },
    });
    await delay();
    assert.equal(
      p.sockets[1].closed,
      true,
      'Reject crop outside the paired source',
    );
    button(p, 'Retry preview').click();
    await until(() => p.sockets.length === 3 && p.sockets[2].sent.length === 1);
    let finishDecode;
    p.decode = () =>
      new Promise((resolve) => {
        finishDecode = resolve;
      });
    const pending = p.sockets[2].message(frame);
    await delay();
    p.previewTimeout();
    await delay();
    assert.equal(
      p.sockets[2].closed,
      true,
      'A stalled decode has a bounded lifetime',
    );
    assert.match(p.element('.preview-state').textContent, /timed out/);
    finishDecode();
    await pending;
    assert.equal(
      p.draws.length,
      0,
      'A timed-out decode cannot draw an obsolete response',
    );
  });
  await check(on, async (p) => {
    let reads = 0;
    p.configGet = async () =>
      response(settingsView(++reads === 1 ? config() : config(45, '2')));
    await p.refresh({ ...on, config_revision: '2', applied_revision: '2' });
    assert.equal(reads, 2);
    settings(p);
    await delay();
    assert.equal(p.element('#processing-shadow_lift_percent').value, '45');
    p.hidden = true;
    const count = p.requests.length;
    await p.refresh();
    assert.equal(p.requests.length, count);
  });
  console.log(
    'Control page checks passed: first-run wizard, full settings, complete documents, deployment separation, validation, conflicts, power/source recovery, preview teardown and coherent polling.',
  );
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
