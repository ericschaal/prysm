import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { previewLayout } from '@prysm/web-portal';
import { httpPreviewClient } from '@prysm/application-client/http';
import { ApplicationError } from '@prysm/application-client';
import { fakePreviewClient } from '../testkit/preview';
import { $, button, renderPortal, sleep } from './dom';

const capture = { kind: 'device', path: '/dev/video0' };
const output = { kind: 'wled', address: '192.0.2.1', leds: { top: 10, left: 4 } };
const red = [255, 0, 0];
const blue = [0, 0, 255];

/** A 4×2 preview of a 1920×1080 frame; `viewport` defaults to the whole frame. */
const frame = (change = {}) => ({
  type: 'frame',
  sequence: 1,
  source_width: 1920,
  source_height: 1080,
  width: 4,
  height: 2,
  viewport: { x: 0, y: 0, width: 1920, height: 1080 },
  colors: { top: [red, blue], right: [blue], bottom: [red], left: [red, red] },
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
      get: (_, name) => (...args) => drawn.push([name, ...args]),
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

test('the preview layout spreads each edge along the picture and maps the viewport', () => {
  const layout = previewLayout(frame({ viewport: { x: 0, y: 270, width: 1920, height: 540 } }));
  assert.deepEqual([layout.width, layout.height], [36, 34]);
  assert.deepEqual(layout.image, { x: 16, y: 16, width: 4, height: 2 });
  const strip = (color) => layout.strips.filter((rect) => rect.color === color);
  // Top: red then blue, left to right, above the picture.
  assert.deepEqual(
    layout.strips.slice(0, 2).map(({ x, y, width, height, color }) => [x, y, width, height, color]),
    [
      [16, 0, 2, 12, red],
      [18, 0, 2, 12, blue],
    ],
  );
  // Right: one colour down the whole right edge.
  assert.deepEqual(
    strip(blue).map(({ x, y, width, height }) => [x, y, width, height]),
    [
      [18, 0, 2, 12],
      [24, 16, 12, 2],
    ],
  );
  assert.deepEqual(layout.viewport, { x: 16, y: 16.5, width: 4, height: 1 });
  assert.equal(previewLayout(frame()).viewport, null);
});

test('the preview streams only while shown and draws the frame on a canvas', async () => {
  const preview = fakePreviewClient();
  const portal = await renderPortal(HeadlessWebPortal, { capture, output }, undefined, {
    preview: preview.client,
  });
  const canvas = fakeCanvas(portal.dom.window);
  const message = () => $('.preview-message')?.textContent;
  try {
    assert.equal(preview.opened(), 0, 'collapsed: no viewer');
    assert.equal(button('Show preview').getAttribute('aria-expanded'), 'false');

    await act(async () => button('Show preview').click());
    assert.equal(preview.open(), 1);
    assert.equal(button('Hide preview').getAttribute('aria-expanded'), 'true');
    assert.equal(message(), 'Connecting to the preview…');

    await act(async () => preview.send({ type: 'idle' }));
    assert.equal(message(), 'Lights are off. Turn on Screen sync to see the picture and its colours.');
    assert.equal($('.preview-canvas').hidden, true);

    await act(async () => preview.send(frame()));
    await flush();
    assert.equal($('.preview-canvas').hidden, false);
    assert.equal($('.preview-info').textContent, 'Picture 1920 × 1080.');
    assert.deepEqual(canvas.decoded, [['image/jpeg', [1, 2, 3]]]);
    assert.equal(canvas.released(), 1);
    assert.equal($('.preview-canvas').width, 36);
    const fills = canvas.drawn.filter(([name]) => name === 'fillStyle=').map(([, value]) => value);
    assert.deepEqual(fills.slice(0, 6), [
      'rgb(255, 0, 0)',
      'rgb(0, 0, 255)',
      'rgb(255, 0, 0)',
      'rgb(255, 0, 0)',
      'rgb(255, 0, 0)',
      'rgb(0, 0, 255)',
    ]);
    assert(canvas.drawn.some(([name, , x, y, w, h]) => name === 'drawImage' && [x, y, w, h].join() === '16,16,4,2'));
    assert(!canvas.drawn.some(([name]) => name === 'strokeRect'), 'no outline for the whole frame');

    // Black bars removed: the sampled area is outlined and named.
    await act(async () =>
      preview.send(frame({ sequence: 2, viewport: { x: 0, y: 140, width: 1920, height: 800 } })),
    );
    await flush();
    assert.equal(
      $('.preview-info').textContent,
      'Picture 1920 × 1080. Black bars removed: colours come from the outlined 1920 × 800 area.',
    );
    assert(canvas.drawn.some(([name, rule]) => name === 'fill' && rule === 'evenodd'));
    assert(canvas.drawn.some(([name]) => name === 'strokeRect'));

    await act(async () => preview.send({ type: 'unavailable', message: 'MJPEG frames cannot be previewed' }));
    assert.equal(message(), 'This picture cannot be shown: MJPEG frames cannot be previewed');

    // Hiding the panel or the page releases the viewer; showing it again reopens.
    await act(async () => button('Hide preview').click());
    assert.equal(preview.open(), 0);
    assert.equal($('.preview-canvas'), null);
    await act(async () => button('Show preview').click());
    assert.equal(preview.open(), 1);
    Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
    await act(async () => document.dispatchEvent(new portal.dom.window.Event('visibilitychange')));
    assert.equal(preview.open(), 0);
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
    await act(async () => document.dispatchEvent(new portal.dom.window.Event('visibilitychange')));
    assert.equal(preview.open(), 1);
    assert.equal(preview.opened(), 3);
  } finally {
    await portal.unmount();
  }
  assert.equal(preview.open(), 0, 'unmounting closes the stream');
  assert.equal(preview.closed(), 3);
});

test('a busy or dropped preview says so, offers a retry and leaves the portal working', async () => {
  const preview = fakePreviewClient();
  const portal = await renderPortal(WebosWebPortal, { capture: { kind: 'webos' }, output }, undefined, {
    preview: preview.client,
  });
  const alert = () => $('.preview [role="alert"] p')?.textContent;
  try {
    preview.refuseNext({ kind: 'busy', message: 'too many preview viewers' });
    await act(async () => button('Show preview').click());
    await flush();
    assert.equal(alert(), 'Too many people are watching the preview. Close it in another window, then try again.');
    assert.equal(preview.open(), 0);

    await act(async () => button('Try again').click());
    assert.equal(preview.open(), 1);
    assert.equal(alert(), undefined);

    await act(async () => preview.end());
    await flush();
    assert.equal(alert(), 'The preview stopped.');
    assert.equal(preview.open(), 0);

    // The rest of the portal is unaffected.
    await act(async () => $('[data-target="processed"]').click());
    await flush();
    assert.equal(portal.fixture.snapshot().target, 'processed');
    await act(async () => button('Try again').click());
    await act(async () => preview.send({ type: 'idle' }));
    assert.equal($('.preview-message').textContent, 'Waiting for the first picture…');
  } finally {
    await portal.unmount();
  }
});

test('without a preview client the portal shows no Preview panel', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    assert.equal($('.preview'), null);
  } finally {
    await portal.unmount();
  }
});

const sse = (text) =>
  new Response(new TextEncoder().encode(text), { headers: { 'content-type': 'text/event-stream' } });

test('the HTTP preview client follows the base URL, decodes events and reports busy', async () => {
  const urls = [];
  const replies = [
    sse(`:\n\ndata: ${JSON.stringify({ type: 'idle' })}\n\ndata: ${JSON.stringify(frame())}\r\n\r\n`),
    new Response(JSON.stringify({ kind: 'busy', message: 'too many preview viewers' }), {
      status: 429,
      headers: { 'content-type': 'application/json' },
    }),
    sse(`data: ${JSON.stringify({ ...frame(), colors: { top: [[256, 0, 0]], right: [], bottom: [], left: [] } })}\n\n`),
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
    (error) => error instanceof ApplicationError && error.failure.kind === 'busy',
  );
  const invalid = await client.preview(new AbortController().signal);
  await assert.rejects(
    invalid.events[Symbol.asyncIterator]().next(),
    (error) => error instanceof ApplicationError && error.failure.message === 'Invalid preview event.',
  );
  assert.deepEqual(urls, Array(3).fill('http://192.168.1.20:8787/api/v1/preview/stream'));
});

test('closing a preview stream cancels the request', async () => {
  let cancelled = 0;
  let signal;
  const client = httpPreviewClient({
    fetch: async (url, init) => {
      signal = init.signal;
      assert.equal(url, '/api/v1/preview/stream');
      return new Response(new ReadableStream({ cancel: () => void cancelled++ }), {
        headers: { 'content-type': 'text/event-stream' },
      });
    },
  });
  const stream = await client.preview(new AbortController().signal);
  const next = stream.events[Symbol.asyncIterator]().next();
  stream.close();
  assert.equal(signal.aborted, true);
  assert((await next.catch(() => ({ done: true }))).done);
  assert(cancelled >= 1);
});
