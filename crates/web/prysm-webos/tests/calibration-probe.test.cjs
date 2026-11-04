'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('jsdom');
const root = path.resolve(__dirname, '../../../..');
const probe = path.join(
  root,
  'research/webos-capture/2026-11-03-calibration-playback/probe',
);
function fixture(t) {
  const dom = new JSDOM(
    fs.readFileSync(path.join(probe, 'index.html'), 'utf8'),
    { runScripts: 'outside-only' },
  );
  t.after(() => dom.window.close());
  const w = dom.window;
  let reject;
  let pauses = 0;
  w.HTMLMediaElement.prototype.play = function () {
    return new Promise((_, failure) => {
      reject = failure;
    });
  };
  w.HTMLMediaElement.prototype.pause = function () {
    pauses += 1;
  };
  w.HTMLMediaElement.prototype.load = function () {};
  w.eval(fs.readFileSync(path.join(probe, 'probe.js'), 'utf8'));
  return {
    w,
    el: (id) => w.document.getElementById(id),
    failure: () => reject(),
    pauses: () => pauses,
  };
}
test('playback hides all UI, Back unloads video, and stale play failures cannot end a new playback', async (t) => {
  const f = fixture(t);
  f.el('start').click();
  const rejectPrevious = f.failure;
  assert.equal(f.el('pattern').getAttribute('src'), 'calibration/response.mp4');
  assert.equal(f.el('menu').hidden, true);
  f.w.document.dispatchEvent(
    new f.w.KeyboardEvent('keydown', { keyCode: 461 }),
  );
  assert.equal(f.el('pattern').hasAttribute('src'), false);
  assert.equal(f.el('menu').hidden, false);
  rejectPrevious();
  f.el('gray').click();
  await Promise.resolve();
  assert.equal(f.el('menu').hidden, true);
  assert.equal(f.el('pattern').getAttribute('src'), 'calibration/gray-192.mp4');
  f.w.dispatchEvent(new f.w.Event('pagehide'));
  assert.equal(f.el('pattern').hasAttribute('src'), false);
  assert.ok(f.pauses() >= 2);
});
test('play and decode failures show a recoverable error and release video', async (t) => {
  const f = fixture(t);
  f.el('start').click();
  f.failure();
  await Promise.resolve();
  assert.match(f.el('status').textContent, /could not play/);
  assert.equal(f.el('menu').hidden, false);
  f.el('start').click();
  f.el('pattern').dispatchEvent(new f.w.Event('error'));
  assert.match(f.el('status').textContent, /decoding failed/);
  assert.equal(f.el('pattern').hasAttribute('src'), false);
});
