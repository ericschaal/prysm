import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  httpApplicationClient,
  httpDiscoveryClient,
} from '@prysm/application-client/http';
import { ApplicationError } from '@prysm/application-client';
import { fakeApplicationPort } from '../testkit/application';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const state = (change = {}) => ({
  ...fakeApplicationPort({
    capture: { kind: 'device', path: '/dev/video0' },
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
  assert.equal(observation.initial.settings.capture.path, '/dev/video0');
  const next = observation.updates.next();
  stream.enqueue(
    encode(
      `data: ${JSON.stringify(state({ target: 'processed', revision: 4 }))}\n\n`,
    ),
  );
  const update = (await next).value;
  assert.equal(update.target, 'processed');
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

test('commands use PUT with JSON bodies and return the runtime result', async () => {
  const requests = [];
  const client = httpApplicationClient({
    fetch: async (url, init) => {
      requests.push([url, init.method, JSON.parse(init.body)]);
      return url.endsWith('/target') ? json(204) : json(200, { revision: 8 });
    },
  });
  await client.setTarget('device_native');
  const settings = state().settings;
  assert.equal(await client.replaceSettings(7, settings), 8);
  assert.deepEqual(requests, [
    ['/api/v1/target', 'PUT', { target: 'device_native' }],
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
  await assert.rejects(client.setTarget('processed'), (error) =>
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
  await client.setTarget('processed');
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
      detail: '192.0.2.40, 120 LEDs',
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
  reply = json(200, [{ label: 'x' }]);
  await assert.rejects(
    client.candidates('capture', 'device'),
    /Invalid candidates reply/,
  );
});
