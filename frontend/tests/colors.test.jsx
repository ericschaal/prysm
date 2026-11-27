import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { httpPortalClient } from '@prysm/application-client/http';
import { ApplicationError, reconnectDelayMs } from '@prysm/application-client';
import { act } from 'react';
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

test('the HTTP colors client follows the base URL, decodes events, skips newer kinds and reports busy', async () => {
  const urls = [];
  const replies = [
    sse(
      `:\n\ndata: ${JSON.stringify({ type: 'idle' })}\n\ndata: ${JSON.stringify({ type: 'later' })}\n\ndata: ${JSON.stringify(colors)}\r\n\r\n`,
    ),
    new Response(
      JSON.stringify({ kind: 'busy', message: 'too many colors streams' }),
      { status: 429, headers: { 'content-type': 'application/json' } },
    ),
    sse(`data: ${JSON.stringify({ ...colors, top: [[256, 0, 0]] })}\n\n`),
  ];
  const client = httpPortalClient({
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
      error instanceof ApplicationError && error.failure.kind === 'internal',
  );
  assert.deepEqual(
    urls,
    Array(3).fill('http://192.168.1.20:8787/api/v1/colors/stream'),
  );
});

test('aborting a colors stream cancels the request', async () => {
  let cancelled = 0;
  let signal;
  const client = httpPortalClient({
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

const capture = { kind: 'device', device: '/dev/video0' };
const output = { kind: 'wled', address: '192.0.2.1', leds: { top: 10 } };
const flush = () => act(async () => sleep(0));
const grid = () => $('.home-grid');
const live = () => grid()?.getAttribute('data-live') ?? null;
const led = (name) => grid().style.getPropertyValue(name);
/** What black LEDs leave on the card. */
const moonlight = 'rgb(61 61 61)';
const publish = (fixture, change) =>
  act(async () => {
    fixture.publish(change);
    await sleep(0);
  });
const running = { target: 'screen_sync', session: { state: 'screen_sync' } };

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
    assert.equal(live(), null);
    assert.equal(led('--live-a'), moonlight, 'moonlight until colours arrive');
    // The LED band is a readout: no colours (neutral) before the first event.
    assert.equal(grid().style.getPropertyValue('--led-t-0'), '');

    await act(async () => fake.send(colors));
    await flush();
    assert.equal(live(), 'colors');
    // Each edge's colours along it, in the stream's order (top left to
    // right), and the card's four light fields, as custom properties.
    assert.equal(led('--led-t-0'), 'rgb(255 0 0)', 'red on the left');
    assert.equal(led('--led-t-15'), 'rgb(0 0 255)', 'blue on the right');
    for (const stop of [0, 15]) {
      assert.equal(led(`--led-r-${stop}`), 'rgb(0 0 255)', 'one colour fills');
      assert.equal(led(`--led-l-${stop}`), 'rgb(255 0 0)');
    }
    for (const pool of ['--live-a', '--live-b', '--live-c', '--live-d'])
      assert.match(
        led(pool),
        /^rgb\(\d+ \d+ \d+\)$/,
        'coloured: full strength',
      );
    const [lr, , lb] = led('--live-a').match(/\d+/g).map(Number);
    const [rr, , rb] = led('--live-b').match(/\d+/g).map(Number);
    assert(lr > lb && rb > rr);

    // The same LEDs dimmed keep their hue and glow lower.
    const sum = (name) =>
      led(name)
        .match(/\d+/g)
        .reduce((total, value) => total + Number(value), 0);
    const full = sum('--live-a');
    const dim = (edge) => edge.map((color) => color.map((value) => value / 4));
    await act(async () =>
      fake.send({
        type: 'colors',
        top: dim(colors.top),
        right: dim(colors.right),
        bottom: dim(colors.bottom),
        left: dim(colors.left),
      }),
    );
    await flush();
    const [dr, , db] = led('--live-a').match(/\d+/g).map(Number);
    assert(dr > db, 'still red');
    assert(sum('--live-a') < full, 'lower');

    // No picture: the moonlight, as for black LEDs.
    await act(async () => fake.send({ type: 'idle' }));
    await flush();
    assert.equal(live(), 'idle');
    assert.equal(led('--live-a'), moonlight);

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
      target: 'effects',
      session: { state: 'effects' },
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

test('a black scene is a colourless moonlight; a refused or ended stream falls back and retries', async () => {
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
    for (const stop of [0, 15]) {
      assert.equal(led(`--led-t-${stop}`), 'rgb(40 40 40)');
      assert.equal(led(`--led-b-${stop}`), 'rgb(40 10 10)');
      assert.equal(led(`--led-r-${stop}`), 'rgb(40 25 25)');
    }
    // The fields by the black top are moonlight; the one by the dim red
    // bottom is red, low.
    for (const pool of ['--live-a', '--live-b', '--live-d'])
      assert.equal(led(pool), moonlight, pool);
    const [lowRed, lowGreen, lowBlue] = led('--live-c')
      .match(/\d+/g)
      .map(Number);
    assert(lowRed > 2 * lowGreen && lowRed > 2 * lowBlue);
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
    for (const pool of ['--live-a', '--live-b', '--live-c', '--live-d'])
      assert.equal(led(pool), moonlight, pool);
    // Grey LEDs give white light, brighter than the moonlight.
    await act(async () =>
      fake.send({
        type: 'colors',
        top: [[128, 128, 128]],
        right: [],
        bottom: [],
        left: [],
      }),
    );
    await flush();
    const grey = led('--live-c').match(/\d+/g).map(Number);
    assert(grey[0] > 100 && Math.max(...grey) - Math.min(...grey) <= 1);

    await act(async () => fake.end());
    await flush();
    assert.equal(live(), null);
    assert.equal(led('--live-a'), moonlight, 'ended: the moonlight');
    assert.equal(fake.open(), 0);
    fake.refuseNext({ kind: 'busy', message: 'too many colors streams' });
    await act(async () => sleep(reconnectDelayMs(0) + 100));
    assert.equal(fake.open(), 0, 'refused: still the moonlight');
    await act(async () => sleep(reconnectDelayMs(1) + 100));
    assert.equal(fake.open(), 1, 'retried quietly');
  } finally {
    await portal.unmount();
  }
});

test('before any colours are written the stylesheet starts the fields in the same moonlight', () => {
  const css = readFileSync(
    new URL('../packages/web-portal/portal.css', import.meta.url),
    'utf8',
  );
  const starts = [
    ...css.matchAll(
      /@property --(?:(?:live|light)-[a-d]|field) \{[^}]*initial-value: ([^;]+);/g,
    ),
  ].map((match) => match[1]);
  assert.equal(starts.length, 9);
  assert(starts.every((value) => value === moonlight));
});

test('without a colors client Home writes no colours: the moonlight stays', async () => {
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

test('a colors stream that finishes opening after Home unmounts is closed unread', async () => {
  const opening = Promise.withResolvers();
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({ ...snapshot, ...running }),
    { colors: { colors: () => opening.promise } },
  );
  await portal.unmount();
  let closed = 0;
  let reads = 0;
  opening.resolve({
    close: () => closed++,
    events: (async function* () {
      reads++;
      yield { type: 'idle' };
    })(),
  });
  await flush();
  assert.equal(closed, 1);
  assert.equal(reads, 0);
});
