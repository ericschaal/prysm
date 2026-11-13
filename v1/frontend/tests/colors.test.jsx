import assert from 'node:assert/strict';
import { test } from 'node:test';
import { httpColorsClient } from '@prysm/application-client/http';
import { ApplicationError } from '@prysm/application-client';
import { act } from 'react';
import { colorsRetryMs } from '@prysm/web-portal';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { fakeColorsClient } from '../testkit/colors';
import { $, button, click, openSettings, renderPortal, sleep } from './dom';

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

const capture = { kind: 'device', path: '/dev/video0' };
const output = { kind: 'wled', address: '192.0.2.1', leds: { top: 10 } };
const flush = () => act(async () => sleep(0));
const grid = () => $('.home-grid');
const live = () => grid()?.getAttribute('data-live') ?? null;
const led = (name) => grid().style.getPropertyValue(name);
const publish = (fixture, change) =>
  act(async () => {
    fixture.publish(change);
    await sleep(0);
  });
const running = { target: 'processed', session: { state: 'running' } };

test('Home streams the LEDs’ colours only while screen sync runs and is seen', async () => {
  const fake = fakeColorsClient();
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    { colors: fake.client },
  );
  const { fixture } = portal;
  try {
    assert.equal(fake.opened(), 0, 'off: no stream');
    await publish(fixture, running);
    assert.equal(fake.open(), 1);
    assert.equal(live(), null, 'the designed glow until colours arrive');

    await act(async () => fake.send(colors));
    await flush();
    assert.equal(live(), 'colors');
    // Each edge's average, and the card's four light fields, as custom properties.
    assert.equal(led('--led-t'), 'rgb(128 0 128)');
    assert.equal(led('--led-l'), 'rgb(255 0 0)');
    for (const pool of ['--live-a', '--live-b', '--live-c', '--live-d'])
      assert.match(
        led(pool),
        /^rgb\(\d+ \d+ \d+\)$/,
        'coloured: full strength',
      );
    // The top-left field comes from red LEDs, the top-right from blue ones.
    const [lr, , lb] = led('--live-a').match(/\d+/g).map(Number);
    const [rr, , rb] = led('--live-b').match(/\d+/g).map(Number);
    assert(lr > lb && rb > rr);

    // No picture: the designed glow, low.
    await act(async () => fake.send({ type: 'idle' }));
    await flush();
    assert.equal(live(), 'idle');

    // Leaving Home, a hidden tab, the lights' own effects or off: closed.
    await openSettings();
    assert.equal(fake.open(), 0);
    await click(button('Home'));
    await flush();
    assert.equal(fake.open(), 1);
    Object.defineProperty(document, 'visibilityState', {
      value: 'hidden',
      configurable: true,
    });
    await act(async () =>
      document.dispatchEvent(new portal.dom.window.Event('visibilitychange')),
    );
    assert.equal(fake.open(), 0);
    Object.defineProperty(document, 'visibilityState', {
      value: 'visible',
      configurable: true,
    });
    await act(async () =>
      document.dispatchEvent(new portal.dom.window.Event('visibilitychange')),
    );
    assert.equal(fake.open(), 1);
    await publish(fixture, {
      target: 'device_native',
      session: { state: 'native' },
    });
    assert.equal(fake.open(), 0, 'effects: one steady colour, no stream');
    assert.equal(live(), null);
    assert.equal($('.light-frame').dataset.palette, 'effects');
    await publish(fixture, running);
    assert.equal(fake.open(), 1);
    await publish(fixture, { target: 'off', session: { state: 'stopped' } });
    assert.equal(fake.open(), 0);
    assert.equal(fake.opened(), fake.closed());
  } finally {
    await portal.unmount();
  }
  assert.equal(fake.open(), 0);
});

test('a black scene glows faintly; a refused or ended stream falls back and retries', async () => {
  const fake = fakeColorsClient();
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({ ...snapshot, ...running }),
    { colors: fake.client },
  );
  try {
    await flush();
    await act(async () =>
      fake.send({
        type: 'colors',
        top: [[0, 0, 0]],
        right: [],
        bottom: [[30, 0, 0]],
        left: [],
      }),
    );
    await flush();
    // The drawing's edges are lifted to a faint floor, hue kept; an edge
    // without LEDs takes the whole.
    assert.equal(led('--led-t'), 'rgb(40 40 40)');
    assert.equal(led('--led-b'), 'rgb(40 10 10)');
    assert.equal(led('--led-r'), 'rgb(40 25 25)');
    // Little colour to give: the pools stay near the designed ones, at full
    // strength (the glow never goes out or muddy).
    for (const pool of ['--live-a', '--live-b', '--live-c', '--live-d'])
      assert.match(led(pool), /^rgb\(\d+ \d+ \d+\)$/, pool);
    await act(async () =>
      fake.send({
        type: 'colors',
        top: [[0, 0, 0]],
        right: [[0, 0, 0]],
        bottom: [[0, 0, 0]],
        left: [[0, 0, 0]],
      }),
    );
    await flush();
    // Black: exactly the designed screen sync light.
    assert.equal(led('--live-a'), 'rgb(224 71 158)');
    assert.equal(led('--live-b'), 'rgb(34 195 238)');
    assert.equal(led('--live-d'), 'rgb(255 138 76)');

    await act(async () => fake.end());
    await flush();
    assert.equal(live(), null, 'ended: the designed glow');
    assert.equal(fake.open(), 0);
    fake.refuseNext({ kind: 'busy', message: 'too many colors streams' });
    await act(async () => sleep(colorsRetryMs(0) + 100));
    assert.equal(fake.open(), 0, 'refused: still the designed glow');
    await act(async () => sleep(colorsRetryMs(1) + 100));
    assert.equal(fake.open(), 1, 'retried quietly');
  } finally {
    await portal.unmount();
  }
});

test('without a colors client Home is unchanged', async () => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({ ...snapshot, ...running }),
  );
  try {
    assert.equal(live(), null);
    assert.equal(grid().getAttribute('style'), null);
  } finally {
    await portal.unmount();
  }
});
