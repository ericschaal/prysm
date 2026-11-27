import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import {
  decodeColors,
  decodePreview,
  readState,
  embeddedState,
  httpApplicationClient,
  httpPortalClient,
} from '@prysm/application-client/http';
import { ApplicationError } from '@prysm/application-client';
import { fakeApplicationPort } from '../testkit/application';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const state = (change = {}) => ({
  ...fakeApplicationPort({
    capture: { kind: 'device', device: '/dev/video0' },
  }).snapshot(),
  ...change,
});
const encode = (text) => new TextEncoder().encode(text);
const json = (status, body) =>
  new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });

test('the state stream decodes split events and comments, sends no credentials and cancels', async () => {
  let stream;
  let cancelled = 0;
  const calls = [];
  const client = httpApplicationClient({
    fetch: async (url, init) => {
      calls.push(url);
      assert.equal(init.headers, undefined);
      return new Response(
        new ReadableStream({
          start(controller) {
            stream = controller;
          },
          cancel() {
            cancelled++;
          },
        }),
        { headers: { 'content-type': 'text/event-stream' } },
      );
    },
  });
  const pending = client.observe(new AbortController().signal);
  await tick();
  const first = encode(`:\n\ndata: ${JSON.stringify(state())}\r\n\r\n`);
  stream.enqueue(first.slice(0, 9));
  stream.enqueue(first.slice(9));
  const observation = await pending;
  assert.equal(observation.initial.settings.capture.device, '/dev/video0');
  const next = observation.updates.next();
  stream.enqueue(
    encode(
      `data: ${JSON.stringify(state({ target: 'screen_sync', revision: 4 }))}\n\n`,
    ),
  );
  const update = (await next).value;
  assert.equal(update.target, 'screen_sync');
  assert.equal(update.revision, 4);
  observation.close();
  assert((await observation.updates.next()).done);
  assert.equal(cancelled, 1);
  assert.deepEqual(calls, ['/api/v1/state/stream']);
});

test('an invalid event ends the stream as internal, asking for a reload', async () => {
  const client = httpApplicationClient({
    fetch: async () =>
      new Response(encode('data: {"kind":"snapshot"}\n\n'), {
        headers: { 'content-type': 'text/event-stream' },
      }),
  });
  await assert.rejects(
    client.observe(new AbortController().signal),
    (error) =>
      error instanceof ApplicationError &&
      error.failure.kind === 'internal' &&
      error.failure.message.endsWith('Reload the page.'),
  );
});

test('a connection lost mid-stream ends the stream as unavailable', async () => {
  let stream;
  const client = httpApplicationClient({
    fetch: async () =>
      new Response(
        new ReadableStream({
          start(controller) {
            stream = controller;
            controller.enqueue(encode(`data: ${JSON.stringify(state())}\n\n`));
          },
        }),
        { headers: { 'content-type': 'text/event-stream' } },
      ),
  });
  const observation = await client.observe(new AbortController().signal);
  const next = observation.updates[Symbol.asyncIterator]().next();
  stream.error(new TypeError('network down'));
  await assert.rejects(
    next,
    (error) =>
      error instanceof ApplicationError && error.failure.kind === 'unavailable',
  );
});

test('a state without its device or version ends the stream as internal', async () => {
  for (const invalid of [
    state({ device: undefined }),
    state({ device: null }),
    state({ version: undefined }),
  ]) {
    const client = httpApplicationClient({
      fetch: async () =>
        new Response(encode(`data: ${JSON.stringify(invalid)}\n\n`), {
          headers: { 'content-type': 'text/event-stream' },
        }),
    });
    await assert.rejects(
      client.observe(new AbortController().signal),
      (error) =>
        error instanceof ApplicationError && error.failure.kind === 'internal',
    );
  }
});

const fixtures = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/contracts/generated/fixtures.json'),
    'utf8',
  ),
);

test('every state, preview and colors event Rust sends decodes as it is', () => {
  for (const [examples, decode] of [
    [fixtures.State, readState],
    [fixtures.PreviewEvent, decodePreview],
    [fixtures.ColorsEvent, decodeColors],
  ])
    for (const example of examples)
      assert.deepEqual(decode(example), { event: example });
});

test('numeric guards keep revisions exact and report values within the wire type', () => {
  assert.equal(
    readState(state({ revision: Number.MAX_SAFE_INTEGER + 1 })),
    'invalid',
  );
  assert.equal(readState(state({ device: { led_count: 2 ** 32 } })), 'invalid');
  assert.equal(
    readState(state({ input: { fps: Infinity, picture: 'arriving' } })),
    'invalid',
  );
  const preview = fixtures.PreviewEvent.find((event) => event.type === 'frame');
  assert.equal(decodePreview({ ...preview, sequence: 2 ** 53 }), 'invalid');
  assert.equal(
    decodePreview({
      ...preview,
      viewport: { ...preview.viewport, x: 2 ** 32 },
    }),
    'invalid',
  );
  const { settings } = state();
  assert.equal(
    readState(
      state({
        settings: {
          ...settings,
          processing: { ...settings.processing, brightness_percent: 256 },
        },
      }),
    ),
    'invalid',
  );
});

test('a snapshot may omit control, but a supplied control must match the wire contract', () => {
  const legacy = state({});
  delete legacy.control;
  assert.deepEqual(readState(legacy), { event: legacy });
  const control = { remembered: 'effects', busy: false, external: false };
  for (const fields of [
    {},
    { waiting: 'capture', power: 'off' },
    { power: 'on', waiting: 'output' },
  ]) {
    const snapshot = state({ control: { ...control, ...fields } });
    assert.deepEqual(readState(snapshot), { event: snapshot });
  }
  for (const invalid of [
    null,
    [],
    {},
    { ...control, remembered: 'off' },
    { ...control, busy: 0 },
    { ...control, external: 'yes' },
    { ...control, waiting: 'other' },
    { ...control, power: null },
  ])
    assert.equal(readState(state({ control: invalid })), 'invalid');
});

test('the snapshot schema rejects malformed data below the top level', () => {
  const { settings } = state();
  for (const malformed of [
    state({ revision: 0.5 }),
    state({ device: { brightness_percent: '35' } }),
    state({ input: { fps: 20 } }),
    state({ surface_failures: [{ kind: 'internal' }] }),
    state({ settings: { ...settings, capture: { device: '/dev/video0' } } }),
    state({
      settings: {
        ...settings,
        processing: { ...settings.processing, style: { kind: 3 } },
      },
    }),
  ])
    assert.equal(readState(malformed), 'invalid');
});

test('saved invalid settings and newer report names are available to the UI', () => {
  const { settings } = state();
  const snapshot = state({
    device: { issue: 'sleeping' },
    input: { fps: 0, picture: 'initializing' },
    session: {
      state: 'stopped',
      failure: {
        kind: 'invalid',
        message: 'Wrong value',
        field: 'processing.brightness_percent',
      },
    },
    settings: {
      // The runtime retains both unfamiliar kinds and invalid adapter fields.
      capture: { kind: 'future', custom: 12 },
      output: { kind: 'wled', address: 42 },
      processing: { ...settings.processing, brightness_percent: 150 },
    },
  });
  assert.deepEqual(readState(snapshot), { event: snapshot });
});

test('newer styles, bottlenecks, calibration patterns and surface failures keep their data', () => {
  const { settings } = state();
  const snapshot = state({
    input: { fps: 20, picture: 'arriving', behind: 'capture' },
    calibration: { pattern: 'new_pattern', uncorrected: false },
    settings: {
      ...settings,
      processing: { ...settings.processing, style: { kind: 'new_style' } },
    },
    surface_failures: [{ kind: 'overheated', message: 'Cooling down' }],
  });
  assert.deepEqual(readState(snapshot), { event: snapshot });
  for (const change of [
    { input: { ...snapshot.input, behind: true } },
    { calibration: { ...snapshot.calibration, pattern: 1 } },
    { surface_failures: [{ kind: 'overheated' }] },
    {
      surface_failures: [
        { kind: 'stale_revision', message: 'Missing revision' },
      ],
    },
  ])
    assert.equal(readState({ ...snapshot, ...change }), 'invalid');
});

test('the state the portal puts in its page is read as a streamed one; none or an invalid one is no state', () => {
  const page = (block) =>
    new JSDOM(`<!doctype html><html><head>${block}</head><body></body></html>`)
      .window.document;
  const embedded = (text) =>
    embeddedState(
      page(`<script type="application/json" id="prysm-state">${text}</script>`),
    );
  // As the portal writes it: markup in a string is escaped.
  const named = state({ version: '1.0.0 </script><!--' });
  const escaped = JSON.stringify(named).replace(
    /[<>&]/g,
    (character) =>
      `\\u${character.charCodeAt(0).toString(16).padStart(4, '0')}`,
  );
  assert.deepEqual(embedded(escaped), named);
  const newer = state({ target: 'ambient' });
  assert.equal(embedded(JSON.stringify(newer)).target, 'unknown');
  assert.equal(embeddedState(page('')), null, 'a dev server');
  assert.equal(embedded(''), null);
  assert.equal(embedded('{"revision":'), null, 'not JSON');
  assert.equal(embedded('{"revision":1}'), null, 'not a state');
});

test('a target or session state newer than the page is read as unknown, never off or stopped', async () => {
  const newer = state({ target: 'ambient', session: { state: 'dimming' } });
  assert.deepEqual(readState(newer), {
    event: { ...newer, target: 'unknown', session: { state: 'unknown' } },
  });
  // A newer state's failure and message are kept, to be shown.
  const failure = { kind: 'adapter', message: 'x' };
  const carrying = state({
    session: { state: 'dimming', failure, message: 'm', extra: 1 },
  });
  assert.deepEqual(readState(carrying), {
    event: {
      ...carrying,
      session: { state: 'unknown', failure, message: 'm' },
    },
  });
  // A faulted session says what did not finish; one without it is malformed.
  const faulted = state({
    session: { state: 'faulted', message: 'Lighting did not stop in time.' },
  });
  assert.deepEqual(readState(faulted), { event: faulted });
  assert.equal(readState(state({ session: { state: 'faulted' } })), 'invalid');
  // Each known state keeps only what it carries: screen sync no failure.
  for (const [sent, read] of [
    [
      { state: 'stopped', failure },
      { state: 'stopped', failure },
    ],
    [
      { state: 'effects', failure },
      { state: 'effects', failure },
    ],
    [{ state: 'effects' }, { state: 'effects' }],
    [{ state: 'screen_sync', failure }, { state: 'screen_sync' }],
  ])
    assert.deepEqual(readState(state({ session: sent })), {
      event: state({ session: read }),
    });
  assert.equal(
    readState(state({ session: { state: 'stopped', failure: 'x' } })),
    'invalid',
  );
  // A known state's failure is whole: `field` and `section` are text.
  for (const broken of [
    { kind: 'invalid', message: 'x', field: 3 },
    { kind: 'adapter', message: 'x', section: false },
    { kind: 'adapter' },
  ])
    assert.equal(
      readState(state({ session: { state: 'stopped', failure: broken } })),
      'invalid',
      JSON.stringify(broken),
    );
  // A side this page does not know is absent; a newer kind is said by its
  // side, else as internal.
  const read = (failure) => {
    const decoded = readState(
      state({ session: { state: 'stopped', failure } }),
    );
    return decoded.event.session.failure;
  };
  assert.deepEqual(read({ kind: 'adapter', message: 'x', section: 'hdmi' }), {
    kind: 'adapter',
    message: 'x',
  });
  assert.deepEqual(
    read({ kind: 'overheated', message: 'x', section: 'output' }),
    {
      kind: 'adapter',
      message: 'x',
      section: 'output',
    },
  );
  assert.deepEqual(read({ kind: 'overheated', message: 'x' }), {
    kind: 'internal',
    message: 'x',
  });
  assert.deepEqual(
    read({ kind: 'overheated', message: 'x', field: 'capture.width' }),
    { kind: 'adapter', message: 'x', section: 'capture' },
    'a field names the side too',
  );
  // A newer state with a malformed failure keeps the state, not the failure.
  assert.deepEqual(
    readState(state({ session: { state: 'dimming', failure: 'x' } })),
    { event: state({ session: { state: 'unknown' } }) },
  );
  assert.equal(readState(state({ target: 3 })), 'invalid');
  assert.equal(readState(state({ session: 'screen_sync' })), 'invalid');
  // Events of a newer kind are skipped; a known kind must be whole.
  assert.equal(decodePreview({ type: 'thumbnail' }), 'unknown');
  assert.equal(decodePreview({ type: 'unavailable' }), 'invalid');
  assert.equal(decodePreview({ kind: 'idle' }), 'invalid');
  assert.equal(decodeColors({ type: 'dimmed' }), 'unknown');
  assert.equal(decodeColors({ type: 'colors', top: [] }), 'invalid');

  const client = httpApplicationClient({
    fetch: async () =>
      new Response(encode(`data: ${JSON.stringify(newer)}\n\n`), {
        headers: { 'content-type': 'text/event-stream' },
      }),
  });
  const observation = await client.observe(new AbortController().signal);
  assert.equal(observation.initial.target, 'unknown');
  assert.deepEqual(observation.initial.session, { state: 'unknown' });
  observation.close();
});

test('commands use PUT with JSON bodies and return the runtime result', async () => {
  const requests = [];
  const client = httpApplicationClient({
    fetch: async (url, init) => {
      requests.push([url, init.method, JSON.parse(init.body)]);
      return url.endsWith('/settings') ? json(200, { revision: 8 }) : json(204);
    },
  });
  await client.setTarget('effects');
  const settings = state().settings;
  assert.equal(await client.tryProcessing(settings.processing), undefined);
  assert.equal(await client.replaceSettings(7, settings), 8);
  assert.deepEqual(requests, [
    ['/api/v1/target', 'PUT', { target: 'effects' }],
    ['/api/v1/processing/trial', 'PUT', settings.processing],
    ['/api/v1/settings', 'PUT', { expected: 7, settings }],
  ]);
});

test('server failures keep their kind, field and current revision', async () => {
  const replies = [
    json(409, { kind: 'stale_revision', message: 'stale', current: 3 }),
    json(422, { kind: 'invalid', message: 'too many', field: 'output.leds' }),
    json(503, { kind: 'faulted', message: 'runtime is faulted' }),
    json(403, { kind: 'forbidden', message: 'origin denied' }),
    new Response('denied', { status: 403 }),
    new Response('gateway', { status: 502 }),
  ];
  const client = httpApplicationClient({ fetch: async () => replies.shift() });
  const failure = async (call) => {
    try {
      await call();
    } catch (error) {
      assert(error instanceof ApplicationError);
      return error.failure;
    }
    assert.fail('expected a failure');
  };
  const settings = state().settings;
  assert.equal(
    (await failure(() => client.replaceSettings(1, settings))).current,
    3,
  );
  assert.equal(
    (await failure(() => client.replaceSettings(3, settings))).field,
    'output.leds',
  );
  assert.equal((await failure(() => client.setTarget('off'))).kind, 'faulted');
  assert.equal(
    (await failure(() => client.setTarget('off'))).message,
    'origin denied',
  );
  assert.equal(
    (await failure(() => client.setTarget('off'))).kind,
    'forbidden',
  );
  assert.equal((await failure(() => client.setTarget('off'))).kind, 'internal');
});

test('a lost reply is unavailable, not a definite rejection', async () => {
  const client = httpApplicationClient({
    fetch: async () => {
      throw new TypeError('network down');
    },
  });
  await assert.rejects(client.setTarget('screen_sync'), (error) =>
    /Check the current state/.test(error.message),
  );
});

test('newer HTTP refusals keep the message and identify a known adapter when possible', async () => {
  for (const [status, body, expected] of [
    [
      422,
      { kind: 'new_error', message: 'Refused' },
      { kind: 'bad_request', message: 'Refused' },
    ],
    [
      500,
      { kind: 'new_error', message: 'Failed' },
      { kind: 'internal', message: 'Failed' },
    ],
    [
      422,
      { kind: 'new_error', message: 'Failed', field: 'capture.path' },
      { kind: 'adapter', section: 'capture', message: 'Failed' },
    ],
    [
      422,
      { kind: 'adapter', message: 'Failed', section: 'another_side' },
      { kind: 'adapter', message: 'Failed' },
    ],
  ]) {
    const client = httpApplicationClient({
      fetch: async () => json(status, body),
    });
    await assert.rejects(client.setTarget('off'), { failure: expected });
  }
});

test('a base URL targets a remote portal for commands and the stream', async () => {
  const urls = [];
  const client = httpApplicationClient({
    baseUrl: 'http://192.168.1.20:8787/',
    fetch: async (url) => {
      urls.push(url);
      if (url.endsWith('/stream'))
        return new Response(encode(`data: ${JSON.stringify(state())}\n\n`), {
          headers: { 'content-type': 'text/event-stream' },
        });
      return url.endsWith('/target') ? json(204) : json(200, { revision: 2 });
    },
  });
  await client.setTarget('screen_sync');
  assert.equal(await client.replaceSettings(1, state().settings), 2);
  const observation = await client.observe(new AbortController().signal);
  assert.equal(observation.initial.target, 'off');
  observation.close();
  assert.deepEqual(urls, [
    'http://192.168.1.20:8787/api/v1/target',
    'http://192.168.1.20:8787/api/v1/settings',
    'http://192.168.1.20:8787/api/v1/state/stream',
  ]);
});

test('candidates are a GET per section and kind, validated', async () => {
  const urls = [];
  const found = [
    {
      label: 'Living room',
      detail: '192.0.2.40',
      led_count: 120,
      fields: { address: '192.0.2.40' },
    },
  ];
  let reply = json(200, found);
  const client = httpPortalClient({
    baseUrl: 'http://192.0.2.9:8787/',
    fetch: async (url, init) => {
      urls.push(url);
      assert.equal(init.method, undefined);
      return reply;
    },
  });
  assert.deepEqual(await client.candidates('output', 'wled'), found);
  assert.deepEqual(urls, [
    'http://192.0.2.9:8787/api/v1/candidates/output/wled',
  ]);

  reply = json(422, {
    kind: 'invalid',
    message: 'unknown kind',
    field: 'output.kind',
  });
  await assert.rejects(client.candidates('output', 'hue'), (error) => {
    assert(error instanceof ApplicationError);
    assert.equal(error.failure.field, 'output.kind');
    return true;
  });
  for (const invalid of [
    [{ label: 'x' }],
    [{ label: 'x', led_count: '120', fields: {} }],
  ]) {
    reply = json(200, invalid);
    await assert.rejects(
      client.candidates('capture', 'device'),
      /Invalid candidates reply/,
    );
  }
});

test('capture modes are posted the capture section and validated', async () => {
  const requests = [];
  const known = {
    status: 'known',
    modes: [
      {
        width: { min: 640, max: 640, step: 1 },
        height: { min: 360, max: 360, step: 1 },
        fps: { min: 1, max: 30, step: 1 },
      },
    ],
  };
  let reply = json(200, known);
  const client = httpPortalClient({
    baseUrl: 'http://192.0.2.9:8787/',
    fetch: async (url, init) => {
      requests.push([url, init.method, JSON.parse(init.body)]);
      return reply;
    },
  });
  const capture = { kind: 'device', device: '/dev/video0' };
  assert.deepEqual(await client.modes(capture), known);
  assert.deepEqual(requests, [
    ['http://192.0.2.9:8787/api/v1/capture/modes', 'POST', capture],
  ]);
  reply = json(200, { status: 'unknown' });
  assert.deepEqual(await client.modes(capture), { status: 'unknown' });
  reply = json(200, { status: 'loading', progress: 10 });
  assert.deepEqual(await client.modes(capture), { status: 'unknown' });

  reply = json(422, {
    kind: 'invalid',
    message: 'unknown kind',
    field: 'capture.kind',
  });
  await assert.rejects(client.modes({ kind: 'hue' }), (error) => {
    assert(error instanceof ApplicationError);
    assert.equal(error.failure.field, 'capture.kind');
    return true;
  });
  for (const invalid of [
    { status: 3 },
    { status: 'known' },
    { status: 'known', modes: [{ width: { min: 1, max: 1, step: 1 } }] },
  ]) {
    reply = json(200, invalid);
    await assert.rejects(client.modes(capture), /Invalid capture modes reply/);
  }
});
