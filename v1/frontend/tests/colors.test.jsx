import assert from 'node:assert/strict';
import { test } from 'node:test';
import { httpColorsClient } from '@prysm/application-client/http';
import { ApplicationError } from '@prysm/application-client';
import { fakeColorsClient } from '../testkit/colors';

const red = [255, 0, 0];
const blue = [0, 0, 255];
const colors = {
  type: 'colors',
  top: [red, blue],
  right: [blue],
  bottom: [red],
  left: [red, red],
};

const sse = (text) =>
  new Response(new TextEncoder().encode(text), {
    headers: { 'content-type': 'text/event-stream' },
  });

test('the HTTP colors client follows the base URL, decodes events and reports busy', async () => {
  const urls = [];
  const replies = [
    sse(
      `:\n\ndata: ${JSON.stringify({ type: 'idle' })}\n\ndata: ${JSON.stringify(colors)}\r\n\r\n`,
    ),
    new Response(
      JSON.stringify({ kind: 'busy', message: 'too many colors streams' }),
      { status: 429, headers: { 'content-type': 'application/json' } },
    ),
    sse(`data: ${JSON.stringify({ ...colors, top: [[256, 0, 0]] })}\n\n`),
  ];
  const client = httpColorsClient({
    baseUrl: 'http://192.168.1.20:8787/',
    fetch: async (url, init) => {
      urls.push(url);
      assert.equal(init.headers, undefined);
      return replies.shift();
    },
  });
  const stream = await client.colors(new AbortController().signal);
  const events = [];
  for await (const event of stream.events) events.push(event);
  assert.deepEqual(events, [{ type: 'idle' }, colors]);
  stream.close();

  await assert.rejects(
    client.colors(new AbortController().signal),
    (error) =>
      error instanceof ApplicationError && error.failure.kind === 'busy',
  );
  const invalid = await client.colors(new AbortController().signal);
  await assert.rejects(
    invalid.events[Symbol.asyncIterator]().next(),
    (error) =>
      error instanceof ApplicationError &&
      error.failure.message === 'Invalid colors event.',
  );
  assert.deepEqual(
    urls,
    Array(3).fill('http://192.168.1.20:8787/api/v1/colors/stream'),
  );
});

test('aborting a colors stream cancels the request', async () => {
  let cancelled = 0;
  let signal;
  const client = httpColorsClient({
    fetch: async (url, init) => {
      signal = init.signal;
      assert.equal(url, '/api/v1/colors/stream');
      return new Response(
        new ReadableStream({ cancel: () => void cancelled++ }),
        { headers: { 'content-type': 'text/event-stream' } },
      );
    },
  });
  const abort = new AbortController();
  const stream = await client.colors(abort.signal);
  const next = stream.events[Symbol.asyncIterator]().next();
  abort.abort();
  assert.equal(signal.aborted, true);
  assert((await next.catch(() => ({ done: true }))).done);
  assert(cancelled >= 1);
});

test('the fake colors client delivers events, counts listeners and refuses', async () => {
  const fake = fakeColorsClient();
  fake.refuseNext({ kind: 'busy', message: 'too many colors streams' });
  await assert.rejects(
    fake.client.colors(new AbortController().signal),
    (error) =>
      error instanceof ApplicationError && error.failure.kind === 'busy',
  );
  const abort = new AbortController();
  const stream = await fake.client.colors(abort.signal);
  assert.equal(fake.open(), 1);
  fake.send({ type: 'idle' });
  fake.send(colors);
  fake.end();
  const events = [];
  for await (const event of stream.events) events.push(event);
  assert.deepEqual(events, [{ type: 'idle' }, colors]);
  assert.equal(fake.open(), 0);
  assert.deepEqual([fake.opened(), fake.closed()], [1, 1]);
});
