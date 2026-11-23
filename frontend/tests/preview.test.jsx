import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { httpPreviewClient } from '@prysm/application-client/http';
import { ApplicationError } from '@prysm/application-client';
import { fakePreviewClient } from '../testkit/preview';
import {
  $,
  assertFocused,
  button,
  click,
  openSettings,
  renderPortal,
  sleep,
} from './dom';

const capture = { kind: 'device', device: '/dev/video0' };
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};

/** A 4×2 preview of a 1920×1080 frame; `viewport` defaults to the whole frame. */
const frame = (change = {}) => ({
  type: 'frame',
  sequence: 1,
  source_width: 1920,
  source_height: 1080,
  width: 4,
  height: 2,
  viewport: { x: 0, y: 0, width: 1920, height: 1080 },
  // Bytes 1, 2, 3.
  jpeg: 'AQID',
  ...change,
});
const flush = () => act(async () => sleep(0));

/** A canvas that records what is drawn, and a decoder that records its input. */
function fakeCanvas(window) {
  const drawn = [];
  const decoded = [];
  let released = 0;
  const context = new Proxy(
    {},
    {
      get:
        (_, name) =>
        (...args) =>
          drawn.push([name, ...args]),
      set: (_, name, value) => {
        drawn.push([`${String(name)}=`, value]);
        return true;
      },
    },
  );
  window.HTMLCanvasElement.prototype.getContext = () => context;
  window.createImageBitmap = async (blob) => {
    decoded.push([blob.type, [...new Uint8Array(await blob.arrayBuffer())]]);
    return { close: () => released++ };
  };
  return { drawn, decoded, released: () => released };
}

const toggle = () => $('.led-map-play');
const line = () => $('.led-map-note').textContent;
const picture = () => $('.led-map-picture canvas');

test('Home’s TV plays the preview in its screen, paused on every visit', async () => {
  const preview = fakePreviewClient();
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    { preview: preview.client },
  );
  const canvas = fakeCanvas(portal.dom.window);
  try {
    // Paused: a play button on the screen, no stream, nothing said.
    assert.equal(toggle().getAttribute('aria-label'), 'Play preview');
    assert.equal(preview.opened(), 0, 'no viewer until played');
    assert.equal(line(), '');
    assert.equal(picture().hidden, true);

    await click(toggle());
    assert.equal(toggle().getAttribute('aria-label'), 'Pause preview');
    assert.equal(preview.open(), 1);
    assert.equal(line(), 'Connecting to the preview…');

    // The picture in the screen; the colours stay on the LED bands.
    await act(async () => preview.send(frame()));
    await flush();
    assert.equal(picture().hidden, false);
    assert.equal(line(), 'Picture 1920 × 1080.');
    assert.deepEqual(canvas.decoded, [['image/jpeg', [1, 2, 3]]]);
    assert.equal(canvas.released(), 1);
    assert.deepEqual([picture().width, picture().height], [4, 2]);
    assert(
      canvas.drawn.some(
        ([name, , x, y, w, h]) =>
          name === 'drawImage' && [x, y, w, h].join() === '0,0,4,2',
      ),
    );
    assert(
      !canvas.drawn.some(([name]) => name === 'strokeRect'),
      'no outline for the whole frame',
    );

    // Black bars removed: the sampled area is outlined and named.
    await act(async () =>
      preview.send(
        frame({
          sequence: 2,
          viewport: { x: 0, y: 140, width: 1920, height: 800 },
        }),
      ),
    );
    await flush();
    assert.equal(
      line(),
      'Picture 1920 × 1080. Colours come from the outlined 1920 × 800 area.',
    );
    assert(
      canvas.drawn.some(
        ([name, rule]) => name === 'fill' && rule === 'evenodd',
      ),
    );
    assert(canvas.drawn.some(([name]) => name === 'strokeRect'));

    // Pause closes the stream; the button keeps the focus.
    toggle().focus();
    await click(toggle());
    assert.equal(preview.open(), 0);
    assert.equal(toggle().getAttribute('aria-label'), 'Play preview');
    assertFocused(toggle());
    assert.equal(line(), '');
    assert.equal(picture().hidden, true);

    // A hidden tab closes it too, and showing the tab reopens it.
    await click(toggle());
    assert.equal(preview.open(), 1);
    const visibility = async (value) => {
      Object.defineProperty(document, 'visibilityState', {
        value,
        configurable: true,
      });
      await act(async () =>
        document.dispatchEvent(new portal.dom.window.Event('visibilitychange')),
      );
    };
    await visibility('hidden');
    assert.equal(preview.open(), 0);
    await visibility('visible');
    assert.equal(preview.open(), 1);

    // Leaving Home closes it; coming back, the preview is paused again.
    await openSettings();
    assert.equal(preview.open(), 0);
    await click(button('Home'));
    assert.equal(toggle().getAttribute('aria-label'), 'Play preview');
    assert.equal(preview.open(), 0);
    await click(toggle());
    assert.equal(preview.open(), 1);
    assert.equal(preview.opened(), 4);
  } finally {
    await portal.unmount();
  }
  assert.equal(preview.open(), 0, 'unmounting closes the stream');
  assert.equal(preview.closed(), 4);
});

test('the card says each state of the preview in one line', async () => {
  const preview = fakePreviewClient();
  const portal = await renderPortal(
    WebosWebPortal,
    { capture: { kind: 'webos' }, output },
    undefined,
    { preview: preview.client },
  );
  try {
    await click(toggle());
    await act(async () => preview.send({ type: 'idle' }));
    assert.equal(
      line(),
      'Lights are off. Turn on Screen sync to see the picture and its colours.',
    );
    await act(async () =>
      portal.fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
      }),
    );
    assert.equal(line(), 'Waiting for the first picture…');

    await act(async () =>
      preview.send({
        type: 'unavailable',
        message: 'MJPEG frames cannot be previewed',
      }),
    );
    assert.equal(
      line(),
      'This picture can’t be shown: MJPEG frames cannot be previewed',
    );

    // Dropped: said, with a retry that opens a new stream.
    await act(async () => preview.end());
    await flush();
    assert.equal(line(), 'The preview stopped. Try again');
    assert.equal(preview.open(), 0);
    await click(button('Try again', $('.led-map-note')));
    assert.equal(preview.open(), 1);
    assertFocused(toggle());
    assert.equal(line(), 'Connecting to the preview…');

    // Too many viewers: said, with a retry.
    await click(toggle());
    preview.refuseNext({ kind: 'busy', message: 'too many preview viewers' });
    await click(toggle());
    await flush();
    assert.equal(line(), 'Too many people are watching the preview. Try again');
    assert.equal(preview.open(), 0);
    await click(button('Try again', $('.led-map-note')));
    assert.equal(preview.open(), 1);

    // Offline: the stream closes and the line says why; back, it plays on.
    await act(async () => portal.fixture.drop());
    assert.equal(preview.open(), 0);
    assert.equal(line(), 'Waiting for the connection…');
    await act(async () => sleep(400));
    assert.equal(preview.open(), 1);
  } finally {
    await portal.unmount();
  }
});

test('without a preview client Home’s TV has no play button', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    assert($('.led-map'), 'the drawing');
    assert(!toggle());
    assert(!$('.led-map-note'));
  } finally {
    await portal.unmount();
  }
});

const sse = (text) =>
  new Response(new TextEncoder().encode(text), {
    headers: { 'content-type': 'text/event-stream' },
  });

test('the HTTP preview client follows the base URL, decodes events, skips newer kinds and reports busy', async () => {
  const urls = [];
  const replies = [
    sse(
      `:\n\ndata: ${JSON.stringify({ type: 'idle' })}\n\ndata: ${JSON.stringify({ type: 'later' })}\n\ndata: ${JSON.stringify(frame())}\r\n\r\n`,
    ),
    new Response(
      JSON.stringify({ kind: 'busy', message: 'too many preview viewers' }),
      {
        status: 429,
        headers: { 'content-type': 'application/json' },
      },
    ),
    sse(`data: ${JSON.stringify(frame({ width: -1 }))}\n\n`),
  ];
  const client = httpPreviewClient({
    baseUrl: 'http://192.168.1.20:8787/',
    fetch: async (url, init) => {
      urls.push(url);
      assert.equal(init.headers, undefined);
      return replies.shift();
    },
  });
  const stream = await client.preview(new AbortController().signal);
  const events = [];
  for await (const event of stream.events) events.push(event);
  assert.deepEqual(events, [{ type: 'idle' }, frame()]);
  stream.close();

  await assert.rejects(
    client.preview(new AbortController().signal),
    (error) =>
      error instanceof ApplicationError && error.failure.kind === 'busy',
  );
  const invalid = await client.preview(new AbortController().signal);
  await assert.rejects(
    invalid.events[Symbol.asyncIterator]().next(),
    (error) =>
      error instanceof ApplicationError &&
      error.failure.message === 'Invalid preview event.',
  );
  assert.deepEqual(
    urls,
    Array(3).fill('http://192.168.1.20:8787/api/v1/preview/stream'),
  );
});

test('closing a preview stream cancels the request', async () => {
  let cancelled = 0;
  let signal;
  const client = httpPreviewClient({
    fetch: async (url, init) => {
      signal = init.signal;
      assert.equal(url, '/api/v1/preview/stream');
      return new Response(
        new ReadableStream({ cancel: () => void cancelled++ }),
        {
          headers: { 'content-type': 'text/event-stream' },
        },
      );
    },
  });
  const stream = await client.preview(new AbortController().signal);
  const next = stream.events[Symbol.asyncIterator]().next();
  stream.close();
  assert.equal(signal.aborted, true);
  assert((await next.catch(() => ({ done: true }))).done);
  assert(cancelled >= 1);
});
