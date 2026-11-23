import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import {
  decodeColors,
  decodePreview,
  decodeState,
  httpApplicationClient,
  httpCaptureModesClient,
  httpDiscoveryClient,
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

test('invalid events end the stream as unavailable', async () => {
  const client = httpApplicationClient({
    fetch: async () =>
      new Response(encode('data: {"kind":"snapshot"}\n\n'), {
        headers: { 'content-type': 'text/event-stream' },
      }),
  });
  await assert.rejects(
    client.observe(new AbortController().signal),
    (error) =>
      error instanceof ApplicationError && error.failure.kind === 'unavailable',
  );
});

test('a state without its device or version ends the stream as unavailable', async () => {
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
        error instanceof ApplicationError &&
        error.failure.kind === 'unavailable',
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
    [fixtures.State, decodeState],
    [fixtures.PreviewEvent, decodePreview],
    [fixtures.ColorsEvent, decodeColors],
  ])
    for (const example of examples)
      assert.deepEqual(decode(example), { event: example });
});

test('a target or session state newer than the page is read as unknown, never off or stopped', async () => {
  // The whole state was refused, so an open page reconnected for ever.
  const newer = state({ target: 'ambient', session: { state: 'dimming' } });
  assert.deepEqual(decodeState(newer), {
    event: { ...newer, target: 'unknown', session: { state: 'unknown' } },
  });
  // A newer state's failure and message are kept, to be shown.
  const failure = { kind: 'adapter', message: 'x' };
  const carrying = state({
    session: { state: 'dimming', failure, message: 'm', extra: 1 },
  });
  assert.deepEqual(decodeState(carrying), {
    event: {
      ...carrying,
      session: { state: 'unknown', failure, message: 'm' },
    },
  });
  // A faulted session says what did not finish; one without it is malformed.
  const faulted = state({
    session: { state: 'faulted', message: 'Lighting did not stop in time.' },
  });
  assert.deepEqual(decodeState(faulted), { event: faulted });
  assert.equal(
    decodeState(state({ session: { state: 'faulted' } })),
    'invalid',
  );
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
    assert.deepEqual(decodeState(state({ session: sent })), {
      event: state({ session: read }),
    });
  assert.equal(
    decodeState(state({ session: { state: 'stopped', failure: 'x' } })),
    'invalid',
  );
  // A known state's failure is whole: `field` and `section` are text.
  for (const broken of [
    { kind: 'invalid', message: 'x', field: 3 },
    { kind: 'adapter', message: 'x', section: false },
    { kind: 'adapter' },
  ])
    assert.equal(
      decodeState(state({ session: { state: 'stopped', failure: broken } })),
      'invalid',
      JSON.stringify(broken),
    );
  // A side this page does not know is absent; a newer kind is said by its
  // side, else as internal.
  const read = (failure) => {
    const decoded = decodeState(
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
    decodeState(state({ session: { state: 'dimming', failure: 'x' } })),
    { event: state({ session: { state: 'unknown' } }) },
  );
  assert.equal(decodeState(state({ target: 3 })), 'invalid');
  assert.equal(decodeState(state({ session: 'screen_sync' })), 'invalid');
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
  assert.equal(
    (await failure(() => client.setTarget('off'))).kind,
    'unavailable',
  );
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
  const client = httpDiscoveryClient({
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
  const client = httpCaptureModesClient({
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
    { status: 'maybe' },
    { status: 'known' },
    { status: 'known', modes: [{ width: { min: 1, max: 1, step: 1 } }] },
  ]) {
    reply = json(200, invalid);
    await assert.rejects(client.modes(capture), /Invalid capture modes reply/);
  }
});
