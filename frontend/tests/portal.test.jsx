import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import {
  connectionGraceMs,
  correctionLimits,
  deviceNamesKey,
  primaryLimits,
  processingDelayMs,
  processingLimits,
  calibrationLimits,
  rebased,
  saveDeveloper,
  styleLimits,
  styleOrder,
  styleValues,
  savedForMs,
  throttled,
  trialIntervalMs,
  ringDelayMs,
  checkForMs,
  outcomeForMs,
  ringHoldMs,
  saveTheme,
  slowStartMs,
  storedTheme,
  applyTheme,
  themeStorageKey,
} from '@prysm/web-portal';
import {
  ledEdgeLimits,
  ledRuns,
  normalizeWledAddress,
  wledUrl,
} from '@prysm/output-wled-ui';
import { includeMenusHelp } from '@prysm/capture-webos-ui';
import { encodingChoices } from '@prysm/capture-remote-ui';
import { defaultProcessing } from '../testkit/application';
import { fakeDiscoveryClient } from '../testkit/discovery';
import { fakePreviewClient } from '../testkit/preview';
import {
  $,
  $$,
  assertFocused,
  assertSame,
  button,
  click,
  edit,
  errorOf,
  field,
  form,
  leaveEditor,
  openEditor,
  openPage,
  openSettings,
  radio,
  row,
  valueOf,
  renderPortal,
  sleep,
  navigated,
  submit,
  text,
  chooseKind,
  kindOf,
  kindRows,
  kinds,
} from './dom';

const capture = { kind: 'device', device: '/dev/video0' };
const unchanged = { hue: 0, strength: 0 };
/** A calibration's correction as the server sends it: every control. */
const noCorrection = {
  warmth: 0,
  tint: 0,
  midtones: 0,
  red: unchanged,
  green: unchanged,
  blue: unchanged,
};
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const mode = (target) => $(`[data-target="${target}"]`);
const power = () => $('[data-power]');
/** The lights' state in one word, as drawn in the light card. */
const word = () => $('.light-word [data-shown="true"]').textContent;
/** The card's one informative line, while it is shown. */
const info = () =>
  $('.light-info').dataset.shown === 'true'
    ? $('.light-info > div > p').textContent
    : null;
/** Home's row of segments named `name`: Style or Calibration. */
const segmentsOf = (name) =>
  $$('.segments-scroll').find(
    (group) =>
      document.getElementById(group.getAttribute('aria-labelledby'))
        .textContent === name,
  );
const chip = (id) => $(`[data-value="${id}"]`, segmentsOf('Calibration'));
const chips = () => $$('.segment', segmentsOf('Calibration'));
/** The card's calm line: what Power will do, a pause that lasts, what runs. */
const note = () => $('.light-calm').textContent || null;
const hint = (element) =>
  document.getElementById(element.getAttribute('aria-describedby')).textContent;
/** The editor's save bar line: a refused save or a conflict. */
const status = () => $('.form-status').textContent;
/** Whether the edited section works now: the status at the editor's top. */
const now = () =>
  [$('.section-status-title'), $('.section-status-detail')]
    .map((line) => line?.textContent ?? '')
    .join(' — ');
const saves = (fixture) =>
  fixture.calls.filter((call) => call === 'replaceSettings').length;
/** Home's address: none when first opened, `#/` when gone back to. */
const atHome = () => ['', '#/'].includes(window.location.hash);
const press = (element) =>
  act(async () => {
    element.click();
    await navigated();
  });
const settle = () => act(async () => sleep(processingDelayMs + 100));
/** Another client replaces the settings. */
const elsewhere = (fixture, change) =>
  act(async () => {
    const { settings, revision } = fixture.snapshot();
    await fixture.client.replaceSettings(revision, { ...settings, ...change });
    await sleep(0);
  });

test('the lighting card says why the lights are not showing screen sync', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
        device: { issue: 'overridden' },
      }),
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Lights follow your screen',
    );
    assert.match(info(), /^Live override is on/);
    assert.equal($('.light-info').dataset.tone, 'warning');
    assert(!$('.light-card [role="alert"]'), 'not a failure');

    await act(async () => fixture.publish({ device: { issue: 'off' } }));
    assert.match(info(), /switched off on the device/);

    await act(async () => fixture.publish({ device: {} }));
    assert.equal(info(), null, 'cleared once the lights follow');
    // It folds away with its last words, out of the tab order.
    assert.equal($('.light-info > div').hasAttribute('inert'), true);
  } finally {
    await portal.unmount();
  }
});

test('a state tick leaves Settings in place; each page has its own save line', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const running = (fps) =>
    act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
        input: { fps, receiving: true, latency: { output_ms: fps / 10 } },
      }),
    );
  try {
    await running(30);
    await openSettings();
    const nodes = () => [
      $('.page-heading h2'),
      $('.save-status'),
      $('.row-link', row('Lights')),
      $('input[type="checkbox"]', row('Remove black bars')),
    ];
    const before = nodes();
    // The engine republishes what it measures about once a second.
    await running(29.9);
    await running(30.1);
    nodes().forEach((node, index) =>
      assertSame(node, before[index], `node ${index} kept across ticks`),
    );
    assert.equal($('.save-status').textContent, 'All changes saved');

    // A save still on its way when Settings is left is not the next page's.
    const release = fixture.holdSaves();
    await click($('input[type="checkbox"]', row('Remove black bars')));
    assert.equal($('.save-status').textContent, 'Saving…');
    await openPage('Style');
    assert($('.page-heading h2') !== before[0], 'a page of its own');
    assert.equal($('.save-status').textContent, 'All changes saved');
    release();
    await act(async () => sleep(50));
    assert.equal(
      $('.save-status').textContent,
      'All changes saved',
      'leaving a save is not saving it',
    );
  } finally {
    await portal.unmount();
  }
});

test('the lighting card names a low WLED brightness while screen sync shows', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    const running = {
      target: 'screen_sync',
      session: { state: 'screen_sync' },
      input: { fps: 30, receiving: true },
    };
    await act(async () =>
      fixture.publish({ ...running, device: { brightness_percent: 40 } }),
    );
    // In place of the calm line, calm itself: a note, not a warning.
    assert.equal(info(), 'WLED limits brightness to 40%.');
    assert.equal($('.light-info').dataset.tone, 'note');
    assert.equal($('.light-calm').dataset.shown, 'false');

    // High enough to hardly see: nothing to say.
    await act(async () =>
      fixture.publish({ device: { brightness_percent: 80 } }),
    );
    assert.equal(info(), null);
    assert.equal($('.light-calm').dataset.shown, 'true');

    // A device that does not show the colours says that instead.
    await act(async () =>
      fixture.publish({
        device: { issue: 'overridden', brightness_percent: 40 },
      }),
    );
    assert.match(info(), /^Live override is on/);

    // WLED's own effects use its brightness anyway: no note.
    await act(async () =>
      fixture.publish({
        target: 'effects',
        session: { state: 'effects' },
        input: undefined,
        device: { brightness_percent: 40 },
      }),
    );
    assert.equal(info(), null);
  } finally {
    await portal.unmount();
  }
});

test('LED counts that differ are said on Home and on the Lights page', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const publish = (change) => act(async () => fixture.publish(change));
  const differ = 'The lights have 120 LEDs, Prysm sends 14.';
  try {
    await publish({
      target: 'screen_sync',
      session: { state: 'screen_sync' },
      input: { fps: 30, receiving: true },
      device: { led_count: 120 },
    });
    assert.equal(info(), differ);
    assert.equal($('.light-info').dataset.tone, 'warning');
    // Before the lights' own brightness: LEDs dark matter more than dim.
    await publish({ device: { led_count: 120, brightness_percent: 40 } });
    assert.equal(info(), differ);
    await publish({ device: { led_count: 14, brightness_percent: 40 } });
    assert.equal(info(), 'WLED limits brightness to 40%.');
    await publish({ device: {} });
    assert.equal(info(), null, 'unknown: nothing to say');

    await publish({ device: { led_count: 120 } });
    await openSettings();
    await openEditor('Lights');
    assert.equal(now(), `LED counts differ — ${differ}`);
    assert.equal($('.section-status').dataset.tone, 'warning');
    const showing = 'Showing screen sync — WLED at 192.0.2.1 · 14\u00a0LEDs';
    await publish({ device: { led_count: 14 } });
    assert.equal(now(), showing);
    await publish({ device: {} });
    assert.equal(now(), showing);

    // What keeps the lights from showing screen sync at all comes first.
    await publish({ device: { led_count: 120, issue: 'unreachable' } });
    assert.match(now(), /^Can’t reach the lights — /);
    await publish({ device: { led_count: 120, issue: 'off' } });
    assert.match(now(), /^Not showing screen sync — /);
    await publish({
      session: {
        state: 'stopped',
        failure: { kind: 'adapter', section: 'output', message: 'no answer' },
      },
      device: { led_count: 120 },
    });
    assert.match(now(), /^Lights not responding — /);
  } finally {
    await portal.unmount();
  }
});

test('Home says when video is awaited; the capture rate is in the technical details', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
        input: { fps: 0, receiving: false },
      }),
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Lights follow your screen',
    );
    // The calm line says it, in the card's kept space, once it lasts.
    assert.equal(info(), null);
    assert.equal(note(), null, 'not at once');
    await act(async () => sleep(slowStartMs + 50));
    assert.equal(info(), null, 'not a problem');
    assert.equal(note(), 'Waiting for the picture.');
    assert.equal(
      $$('.path-fact')[0].textContent,
      'Waiting for picture',
      'in place of the Video fact',
    );

    await act(async () =>
      fixture.publish({ input: { fps: 29.9, receiving: true } }),
    );
    assert.equal(info(), null);
    assert.equal(note(), 'Following the screen.');
    assert(
      !$('.light-card').textContent.includes('fps'),
      'no live frame rate on Home',
    );
    await openPage('About this Prysm');
    assert.equal(valueOf('Frames per second'), '30');
    assert.equal(valueOf('Prysm version'), '1.0.0');
    await click(button('Settings'));
    await click(button('Home'));

    await act(async () =>
      fixture.publish({
        target: 'off',
        session: { state: 'stopped' },
        input: undefined,
      }),
    );
    assert.equal($('.lighting-status').textContent, 'Lights are off');
  } finally {
    await portal.unmount();
  }
});

test('the power button and the mode tiles set the target and show what the session is doing', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const targets = () =>
    fixture.calls.filter((call) => call.startsWith('target:'));
  try {
    // The mode: a radio group of two option cards, each saying what it does.
    assert.equal($('[role="radiogroup"]').getAttribute('aria-label'), 'Mode');
    assert.deepEqual(
      $$('.modes [role="radio"]').map((item) => [
        $('.mode-name', item).textContent,
        $('.mode-line', item).textContent,
        item.getAttribute('aria-checked'),
        item.tabIndex,
      ]),
      [
        ['Screen sync', 'Follows the screen', 'true', 0],
        ['WLED effects', 'Patterns from WLED', 'false', -1],
      ],
    );
    assert.equal(power().getAttribute('aria-pressed'), 'false');
    assert.equal(word(), 'Off');
    assert.equal($('.lighting-status').textContent, 'Lights are off');
    assert.equal($('.light-card').dataset.mood, 'off');
    assert.equal(
      $('.modes').dataset.mode,
      'screen_sync',
      'power turns on sync',
    );
    assert.equal($('.light-card').dataset.lit, 'false');
    assert.equal(info(), null, 'no sentence in the normal states');

    // Power turns screen sync on; the light, and the word, come with the session.
    await press(power());
    assert.deepEqual(targets(), ['target:screen_sync']);
    assert.equal(power().getAttribute('aria-pressed'), 'true');
    assert.equal(mode('screen_sync').getAttribute('aria-checked'), 'true');
    assert.equal(word(), 'On');
    assert.equal($('.light-card').dataset.mood, 'sync');
    assert.equal($('.light-card').dataset.lit, 'true');
    assert.equal(info(), null);
    assert.equal(
      $('.lighting-status').textContent,
      'Lights follow your screen',
    );

    // The tile that is already on restarts nothing.
    await press(mode('screen_sync'));
    assert.equal(targets().length, 1);

    // The other option switches mode while on. Prysm's picture controls do
    // not reach the lights' own effects: they stay in place, quiet and out of
    // reach, and the card's line says so with the way to WLED.
    await press(mode('effects'));
    assert.deepEqual(targets().slice(-1), ['target:effects']);
    assert.equal($('.lighting-status').textContent, 'WLED effects are on');
    assert.equal($('.light-card').dataset.mood, 'effects');
    assert.equal(mode('effects').getAttribute('aria-checked'), 'true');
    const controls = $('.picture-controls');
    assert.equal(controls.hasAttribute('inert'), true);
    assert.equal(controls.dataset.quiet, 'true');
    assert.match(
      $('.light-calm').textContent,
      /^WLED controls its own effects\./,
    );
    const link = $('.light-calm .device-link');
    assert.equal(link.href, 'http://192.0.2.1/');
    assert.equal(link.target, '_blank');
    assert.match(link.rel, /\bnoopener\b/);
    assert.match(link.textContent, /^Open WLED/);

    // Off, then on again: power brings back the last mode.
    await press(power());
    assert.deepEqual(targets().slice(-1), ['target:off']);
    assert.equal(word(), 'Off');
    assert.equal($('.lighting-status').textContent, 'Lights are off');
    assert.equal($('.modes').dataset.mode, 'effects');
    await press(power());
    assert.deepEqual(targets().slice(-1), ['target:effects']);
    await press(power());

    // A tile chosen while off turns that mode on.
    await press(mode('screen_sync'));
    assert.deepEqual(targets().slice(-2), ['target:off', 'target:screen_sync']);
    assert.equal(word(), 'On');

    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: {
          state: 'stopped',
          failure: {
            kind: 'adapter',
            message: 'WLED at 192.0.2.1 did not switch to live mode',
          },
        },
      }),
    );
    assert.equal($('.lighting-status').textContent, 'Screen sync stopped');
    assert($('.lighting-status').classList.contains('visually-hidden'));
    assert.equal($$('.light-card .notice').length, 0, 'one sentence, no box');
    // Plain words from the typed failure; the message under Details.
    assert.equal(info(), 'Screen sync couldn’t start.');
    assert.equal(word(), 'Stopped');
    assert.equal(
      $('.light-card [role="alert"]').textContent,
      'Screen sync couldn’t start.',
    );
    assert.equal(
      $('.light-details p').textContent,
      'WLED at 192.0.2.1 did not switch to live mode',
    );
    assert.equal(power().getAttribute('aria-pressed'), 'true', 'still on');

    // After a failure Try again, or the screen sync tile, retries.
    fixture.failNext({ kind: 'adapter', message: 'fake output cannot open' });
    await press(button('Try again'));
    assert.deepEqual(targets().slice(-1), ['target:screen_sync']);
    assert(text().includes('fake output cannot open'));
    fixture.failNext({ kind: 'adapter', message: 'still cannot open' });
    await press(mode('screen_sync'));
    assert.equal(targets().length, 8);
    assert(text().includes('still cannot open'));

    await act(async () =>
      fixture.publish({
        session: {
          state: 'faulted',
          message: 'The video source did not close in time.',
        },
      }),
    );
    assert.equal($('.lighting-status').textContent, 'Prysm stopped responding');
    assert.equal(info(), 'Prysm stopped responding. Restart Prysm to recover.');
    assert.equal(
      $('.light-details p').textContent,
      'The video source did not close in time.',
    );
    assert($$('[data-target]').every((item) => item.disabled));
    assert.equal(power().disabled, true);
    assert.equal(hint(power()), 'Restart Prysm to change the lights.');
  } finally {
    await portal.unmount();
  }
});

test('the word follows the light; starting is said only when it lasts', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    // The engine accepts the target; the session follows later.
    await act(async () =>
      fixture.publish({ target: 'screen_sync', session: { state: 'stopped' } }),
    );
    assert.equal(power().getAttribute('aria-pressed'), 'true');
    assert.equal(word(), 'Off', 'no "On" before the light');
    assert.equal($('.light-card').dataset.mood, 'starting');
    assert.equal(info(), null, 'a quick start says nothing');
    assert.equal($('.light-card').dataset.ignite, undefined, 'not yet lit');
    await act(async () => sleep(slowStartMs + 50));
    assert.equal(info(), null, 'not a problem');
    assert.equal(note(), 'Starting…');

    await act(async () =>
      fixture.publish({ session: { state: 'screen_sync' } }),
    );
    assert.equal(word(), 'On');
    assert.equal(info(), null);
    // The ignition plays as the light arrives.
    assert.equal($('.light-card').dataset.ignite, 'a');

    // Switching modes passes through starting without the light going out.
    await act(async () =>
      fixture.publish({
        target: 'effects',
        session: { state: 'stopped' },
      }),
    );
    assert.equal($('.light-card').dataset.lit, 'true');
    assert.equal(word(), 'On');
    await act(async () => sleep(slowStartMs + 50));
    assert.equal(info(), null, 'still lit: no problem to say');
    // The picture controls go quiet for the lights' own effects, and say so.
    assert.match(note(), /^WLED controls its own effects\./);
    await act(async () => fixture.publish({ session: { state: 'effects' } }));
    assert.equal($('.light-frame').dataset.palette, 'effects');

    assert.equal(
      $('.light-card').dataset.ignite,
      'a',
      'a mode switch is no ignition',
    );

    // Off: the light goes, and with it the word.
    await act(async () =>
      fixture.publish({ target: 'off', session: { state: 'stopped' } }),
    );
    assert.equal($('.light-card').dataset.lit, 'false');
    assert.equal(word(), 'Off');
    // On again: the ignition's other name restarts it.
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
      }),
    );
    assert.equal($('.light-card').dataset.ignite, 'b');
  } finally {
    await portal.unmount();
  }
});

test('a start that fails after switching from effects shows Stopped, unlit, in the error light', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    // The lights run their own effects.
    await act(async () =>
      fixture.publish({
        target: 'effects',
        session: { state: 'effects' },
      }),
    );
    assert.equal(word(), 'On');
    assert.equal($('.light-frame').dataset.palette, 'effects');
    // As the engine does (`set_target` then a failed `start`): first the
    // target changes while the effects still show …
    await act(async () => fixture.publish({ target: 'screen_sync' }));
    assert.equal($('.light-card').dataset.mood, 'starting');
    // … then the failure is recorded in the session, still the effects.
    await act(async () =>
      fixture.publish({
        session: {
          state: 'effects',
          failure: {
            kind: 'adapter',
            message:
              'video device capture failed: capture device does not support 640x360 at 30 Hz',
          },
        },
      }),
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Screen sync couldn’t start',
    );
    assert.equal(fixture.snapshot().session.state, 'effects');
    assert.equal(word(), 'Stopped');
    assert.equal($('.light-card').dataset.lit, 'false');
    assert.equal($('.light-card').dataset.mood, 'problem');
    assert.equal(
      $('.light-frame').dataset.palette,
      'error',
      'the warning lamp',
    );
    assert.equal(
      info(),
      'Screen sync couldn’t start. WLED effects are still on.',
    );
    assert.match($('.light-details p').textContent, /does not support 640x360/);
    assert(button('Try again'));
    // Settings Prysm can't use are said as such, though a field names the
    // video source: they do not validate, or the source can't deliver them,
    // and a setting is the fix.
    await act(async () =>
      fixture.publish({
        session: {
          state: 'effects',
          failure: {
            kind: 'invalid',
            field: 'capture.width',
            message: 'unsupported size',
          },
        },
      }),
    );
    assert.equal(
      info(),
      'Prysm can’t use these settings. WLED effects are still on.',
    );
    // Off clears it.
    await act(async () =>
      fixture.publish({
        target: 'off',
        session: { state: 'stopped' },
      }),
    );
    assert.equal(word(), 'Off');
    assert.notEqual($('.light-frame').dataset.palette, 'error');
  } finally {
    await portal.unmount();
  }
});

test('a brightness save while stopped keeps Stopped, the red light and Try again', async () => {
  const capture = { kind: 'device', device: '/dev/video0' };
  const failure = {
    kind: 'adapter',
    message:
      'video device capture failed: capture device does not support 640x360 at 30 Hz',
  };
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const stopped = () => {
    assert.equal(word(), 'Stopped');
    assert.equal($('.light-card').dataset.lit, 'false');
    assert.equal($('.light-frame').dataset.palette, 'error');
    assert.equal(power().getAttribute('aria-busy'), 'false', 'no busy ring');
    assert.equal(
      info(),
      'Screen sync couldn’t start. WLED effects are still on.',
    );
    assert(button('Try again'));
  };
  try {
    await act(async () =>
      fixture.publish({
        target: 'effects',
        session: { state: 'effects' },
      }),
    );
    await act(async () => fixture.publish({ target: 'screen_sync' }));
    await act(async () =>
      fixture.publish({ session: { state: 'effects', failure } }),
    );
    stopped();
    // The save (as the engine now does it): new settings and revision, then
    // the retry's result. The failure is never cleared in between.
    await edit(field('processing.brightness_percent'), 75);
    await settle();
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 75);
    await act(async () =>
      fixture.publish({
        session: { state: 'effects', failure: { ...failure } },
      }),
    );
    await act(async () => sleep(slowStartMs + 100));
    stopped();
    assert(!text().includes('are on'), 'no session sentence');

    // Starting from effects that takes a while says what is starting.
    await act(async () => fixture.publish({ session: { state: 'effects' } }));
    assert.equal($('.light-card').dataset.mood, 'starting');
    await act(async () => sleep(slowStartMs + 100));
    assert.equal(note(), 'Starting…');
    assert(!text().includes('WLED effects are on'));

    // Off with a failure of the switch off itself: said, with no retry.
    await act(async () =>
      fixture.publish({
        target: 'off',
        session: {
          state: 'stopped',
          failure: { kind: 'adapter', message: 'WLED refused the connection' },
        },
      }),
    );
    assert.equal(word(), 'Off');
    assert.equal(info(), 'The lights couldn’t be switched off.');
    assert(!button('Try again'));
  } finally {
    await portal.unmount();
  }
});

test('the busy ring shows only for a slow operation, after a delay, and is held', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const ring = () => power().dataset.ring;
  const seen = [];
  const watch = (ms) =>
    act(async () => {
      const until = Date.now() + ms;
      while (Date.now() < until) {
        seen.push(ring());
        await sleep(20);
      }
    });
  try {
    // A quick operation: no ring at all.
    await act(async () => power().click());
    await watch(ringDelayMs + ringHoldMs + 100);
    assert(!seen.includes('true'), 'never shown for a quick operation');
    assert.equal(word(), 'On');

    // A slow one (500 ms): shown after the delay, held past its end.
    const setTarget = fixture.client.setTarget;
    fixture.client.setTarget = async (target) => {
      await sleep(500);
      return setTarget(target);
    };
    const started = Date.now();
    await act(async () => power().click());
    await act(async () => sleep(ringDelayMs - 150));
    assert.equal(ring(), 'false', 'not before the delay');
    await act(async () => sleep(250));
    assert.equal(ring(), 'true', 'shown once slow');
    // The operation ends at 500 ms; the ring stays to delay + hold.
    await act(async () =>
      sleep(Math.max(0, started + ringDelayMs + ringHoldMs - 120 - Date.now())),
    );
    assert.equal(word(), 'Off', 'the operation is done');
    assert.equal(ring(), 'true', 'held for the minimum');
    await act(async () => sleep(300));
    assert.equal(ring(), 'false', 'then it goes');
  } finally {
    await portal.unmount();
  }
});

test('modes that cannot work are disabled and say why; before setup only the prompt shows', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture,
    output: { kind: 'null' },
  });
  try {
    assert.equal(mode('screen_sync').disabled, false);
    assert.equal(mode('effects').disabled, true);
    assert.equal(
      hint(mode('effects')),
      'These lights have no effects of their own.',
    );
    assert.equal(
      $('.mode-name', mode('effects')).textContent,
      'Device effects',
    );
    assert.equal(
      $('.mode-line', mode('effects')).textContent,
      'Its own patterns',
    );
    assert.equal(power().disabled, false);
    assert(!$('.device-link'), 'no device to open');
  } finally {
    await portal.unmount();
  }
  for (const initial of [{ output }, { capture }, {}]) {
    const unset = await renderPortal(HeadlessWebPortal, initial);
    try {
      await click(button('Leave setup'));
      // A calm card: the prompt and its one action, nothing disabled to puzzle over.
      assert.equal($$('[data-target]').length, 0);
      assert(!power());
      assert(!$('.light-card input'));
      assert(!$('.segment'));
      assert.equal(button('Set up Prysm').disabled, false);
      assert(!$('.setup-card'));
      assert.equal(word(), 'Off');
    } finally {
      await unset.unmount();
    }
  }
});

test('a stale picture change keeps both changes: mine where I changed, theirs elsewhere', () => {
  const base = { ...defaultProcessing, brightness_percent: 80 };
  const custom_style = { ...styleValues.natural, dark_floor_percent: 10 };
  const mine = {
    ...base,
    style: { kind: 'custom' },
    custom_style,
    brightness_percent: 60,
  };
  const theirs = { ...base, sample_density: 200, brightness_percent: 90 };
  assert.deepEqual(rebased(theirs, base, mine), {
    ...base,
    style: { kind: 'custom' },
    custom_style,
    sample_density: 200,
    // The same control on both: the value the user sees wins.
    brightness_percent: 60,
  });
  // A removed list stays removed; an untouched one follows theirs.
  const calibrations = [{ id: 'a', name: 'A' }];
  assert.deepEqual(
    rebased({ ...base, calibrations }, { ...base, calibrations }, base),
    base,
  );
  assert.deepEqual(rebased({ ...base, calibrations }, base, base), {
    ...base,
    calibrations,
  });
});

test('Settings has one save status; a refused stale change is saved again on the newer settings', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const status = () => $('.save-status').textContent;
  try {
    await openSettings();
    assert.equal(status(), 'All changes saved');
    await openPage('Detail');
    assert.equal(window.location.hash, '#/settings/detail');
    assert.equal(status(), 'All changes saved');
    await edit(field('processing.sample_density'), 100);
    assert.equal(status(), 'Saving…');
    await settle();
    assert.equal(status(), 'Saved');
    await act(async () => sleep(savedForMs));
    assert.equal(status(), 'All changes saved');

    // Another device saved meanwhile; this page has not heard yet.
    fixture.failNext({
      kind: 'stale_revision',
      message: 'settings revision 1 is stale; current is 2',
      current: 2,
    });
    await edit(field('processing.sample_density'), 150);
    await settle();
    assert.equal(saves(fixture), 2);
    assert.equal(status(), 'Saving…', 'waits for the newer settings');
    await elsewhere(fixture, {
      processing: {
        ...fixture.snapshot().settings.processing,
        style: { kind: 'calm' },
      },
    });
    await settle();
    const saved = fixture.snapshot().settings.processing;
    assert.equal(saved.sample_density, 150);
    assert.deepEqual(saved.style, { kind: 'calm' });
    assert.equal(status(), 'Saved');
    assert(!text().includes('changed elsewhere'));
    await leaveEditor();
    await openPage('Style');
    assert.equal(radio('Calm').checked, true);
  } finally {
    await portal.unmount();
  }
});

test('a picture save that fails is shown at its control, with a retry there', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await openSettings();
    const toggle = field('processing.remove_black_bars');
    const row = () => toggle.closest('.field');
    fixture.failNext({
      kind: 'unavailable',
      message: 'Prysm didn’t answer.',
    });
    await click(toggle);
    await settle();
    assert.equal(errorOf(toggle), 'Not saved. Try again');
    assert.equal($('.save-status').textContent, 'Not saved');
    await settle();
    assert.equal(saves(fixture), 1, 'not retried by itself');

    await click(button('Try again', row()));
    await settle();
    assert.equal(saves(fixture), 2);
    assert.equal(
      fixture.snapshot().settings.processing.remove_black_bars,
      true,
    );
    assert.equal(errorOf(toggle), undefined);
    assert.equal($('.save-status').textContent, 'Saved');
  } finally {
    await portal.unmount();
  }
});

test('arrow keys move what the lights follow, as in a radio group', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture, dom } = portal;
  const key = (element, name) =>
    act(async () => {
      element.dispatchEvent(
        new dom.window.KeyboardEvent('keydown', { key: name, bubbles: true }),
      );
      await sleep(0);
    });
  try {
    await key(mode('screen_sync'), 'ArrowRight');
    assert.equal(fixture.snapshot().target, 'effects');
    assertFocused(mode('effects'));
    assert.equal(mode('effects').tabIndex, 0);
    assert.equal(mode('screen_sync').tabIndex, -1);
    await key(mode('effects'), 'ArrowLeft');
    assert.equal(fixture.snapshot().target, 'screen_sync');
    assertFocused(mode('screen_sync'));
  } finally {
    await portal.unmount();
  }
});

test('Open WLED derives http://host[:port]/ from the saved address', () => {
  for (const [address, url] of [
    ['192.168.1.50', 'http://192.168.1.50/'],
    ['wled.local', 'http://wled.local/'],
    ['WLED.local:8080', 'http://wled.local:8080/'],
    ['192.168.1.50:80', 'http://192.168.1.50/'],
    ['[fe80::1]:8080', 'http://[fe80::1]:8080/'],
    ['fe80::1', 'http://[fe80::1]/'],
    ['2001:db8::7', 'http://[2001:db8::7]/'],
    [' http://wled.local/ ', 'http://wled.local/'],
    ['', null],
    ['wled.local/path', null],
    ['user@wled.local', null],
    ['https://wled.local', null],
  ])
    assert.equal(wledUrl(address), url, address);
  for (const [typed, address] of [
    [' http://wled.local:8080/ ', 'wled.local:8080'],
    // Port 80 is WLED's own: the same lights as discovery names them.
    ['192.168.1.50:80', '192.168.1.50'],
    ['http://wled.local:80/', 'wled.local'],
    ['192.168.1.50:8080', '192.168.1.50:8080'],
    ['[fe80::1]:80', '[fe80::1]:80'],
    ['2001:db8::80', '2001:db8::80'],
  ])
    assert.equal(normalizeWledAddress(typed), address, typed);
});

test('Settings keeps brightness and the calibration choice on Home; the picture saves live', async () => {
  const calibrations = [{ id: 'ps5', name: 'PS5', correction: noCorrection }];
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          calibrations,
          active_calibration: 'ps5',
        },
      },
    }),
  );
  const { fixture } = portal;
  const control = (name) => field(`processing.${name}`);
  const attributes = (element) =>
    ['type', 'value', 'min', 'max', 'step'].map((name) => element[name] ?? '');
  try {
    await openSettings();
    // Only black bars here: brightness and the calibration choice live on Home.
    assert(!control('brightness_percent'));
    assert(!control('active_calibration'));
    assert.equal(control('remove_black_bars').checked, false);
    assert.equal(valueOf('Calibrations'), '1 calibration · PS5 in use');
    await click(control('remove_black_bars'));
    await settle();
    assert.equal(saves(fixture), 1);
    assert.equal(
      fixture.snapshot().settings.processing.remove_black_bars,
      true,
    );

    // Detail stays outside the styles, on a page of its own.
    await openPage('Detail');
    assert.deepEqual(attributes(control('sample_density')), [
      'range',
      '1',
      '1',
      '512',
      '1',
    ]);
    assert.equal(control('sample_density').labels[0].textContent, 'Detail');
    // Two quick changes are applied once, after the user stops.
    await edit(control('sample_density'), 64);
    await edit(control('sample_density'), 128);
    assert.equal(saves(fixture), 1);
    await settle();
    assert.equal(saves(fixture), 2);
    assert.equal(fixture.snapshot().settings.processing.sample_density, 128);

    // One row per style, the saved one ticked; the sliders closed under them.
    await leaveEditor();
    await openPage('Style');
    assert.deepEqual(
      $$('.row-choice').map((choice) => [
        $('.row-label', choice).textContent,
        $('.row-value', choice).textContent,
      ]),
      [
        ['Natural', 'Calm in quiet scenes, follows cuts at once.'],
        ['Calm', 'A slow, soft glow. Nothing sudden.'],
        ['Vivid', 'Stronger colours, follows the action.'],
        ['Game', 'Reacts at once, with the least delay.'],
        ['Custom', 'Your own values.'],
      ],
    );
    assert.equal(radio('Natural').checked, true);
    const adjust = () => $('button', row('Adjust this style'));
    assert.equal(adjust().getAttribute('aria-expanded'), 'false');
    assert.equal(
      valueOf('Adjust this style'),
      'Moving a slider switches to Custom.',
    );
    assert.equal($$('input[type="range"]').length, 0);

    // A style applies at once, by name only.
    await click(radio('Calm'));
    assert.equal(saves(fixture), 3);
    assert.deepEqual(fixture.snapshot().settings.processing.style, {
      kind: 'calm',
    });
    assert.equal(
      fixture.snapshot().settings.processing.custom_style,
      undefined,
    );
    assert.equal(radio('Calm').checked, true);

    // The first Custom starts from the style that was ticked, and opens the
    // sliders.
    await click(radio('Custom'));
    assert.equal(saves(fixture), 4);
    assert.equal(adjust().getAttribute('aria-expanded'), 'true');
    assert.deepEqual(
      [
        fixture.snapshot().settings.processing.style,
        fixture.snapshot().settings.processing.custom_style,
      ],
      [{ kind: 'custom' }, styleValues.calm],
    );
    const knob = (name) => control(`custom_style.${name}`);
    const knobs = [
      'smoothing_seconds',
      'cut_response_percent',
      'brightness_ramp_seconds',
      'spatial_reach_percent',
      'color_focus_percent',
      'saturation_boost_percent',
      'dark_floor_percent',
    ];
    // In the page's order: timing first, then where and how colour is taken.
    assert.deepEqual(
      $$('input[type="range"]').map((input) => input.name),
      knobs.map((name) => `processing.custom_style.${name}`),
    );
    assert.deepEqual(
      knobs.map((name) => attributes(knob(name))),
      [
        // Smoothing as a slider over the range worth tuning.
        ['range', '0.8', '0', '2', '0.05'],
        ['range', '0', '0', '100', '1'],
        ['range', '1', '0', '2', '0.1'],
        ['range', '20', '1', '100', '1'],
        ['range', '0', '0', '100', '1'],
        ['range', '0', '0', '100', '1'],
        ['range', '0', '0', '20', '1'],
      ],
    );
    assert.deepEqual(
      knobs.map((name) => knob(name).labels[0].textContent),
      [
        'Smoothing',
        'Scene cuts',
        'Brightness ramp',
        'Edge depth',
        'Colour focus',
        'Colour boost',
        'Dark-scene glow',
      ],
    );

    // The sliders write the user's own values, after the user stops.
    await edit(knob('color_focus_percent'), 40);
    assert.equal(
      $('output', knob('color_focus_percent').closest('.field')).textContent,
      '40%',
    );
    await edit(knob('saturation_boost_percent'), 25);
    await edit(knob('dark_floor_percent'), 8);
    assert.equal(
      $('output', knob('dark_floor_percent').closest('.field')).textContent,
      '8%',
    );
    await edit(knob('smoothing_seconds'), 0.5);
    assert.equal(
      $('output', knob('smoothing_seconds').closest('.field')).textContent,
      '0.5 s',
    );
    await edit(knob('cut_response_percent'), 60);
    await edit(knob('brightness_ramp_seconds'), 0.5);
    await edit(knob('spatial_reach_percent'), 30);
    assert.equal(saves(fixture), 4);
    await settle();
    assert.equal(saves(fixture), 5);
    const mine = {
      smoothing_seconds: 0.5,
      cut_response_percent: 60,
      brightness_ramp_seconds: 0.5,
      spatial_reach_percent: 30,
      color_focus_percent: 40,
      saturation_boost_percent: 25,
      dark_floor_percent: 8,
    };
    assert.deepEqual(fixture.snapshot().settings.processing, {
      ...defaultProcessing,
      style: { kind: 'custom' },
      custom_style: mine,
      sample_density: 128,
      remove_black_bars: true,
      calibrations,
      active_calibration: 'ps5',
    });

    // Another style keeps them, and the open sliders show its own values;
    // Custom returns to the kept ones.
    await click(radio('Vivid'));
    assert.equal(adjust().getAttribute('aria-expanded'), 'true');
    assert.deepEqual(fixture.snapshot().settings.processing.style, {
      kind: 'vivid',
    });
    assert.deepEqual(fixture.snapshot().settings.processing.custom_style, mine);
    assert.deepEqual(
      knobs.map((name) => Number(knob(name).value)),
      knobs.map((name) => styleValues.vivid[name]),
    );
    await click(radio('Custom'));
    assert.deepEqual(fixture.snapshot().settings.processing.custom_style, mine);
    assert.deepEqual(
      knobs.map((name) => Number(knob(name).value)),
      knobs.map((name) => mine[name]),
    );
    await leaveEditor();
    assert.equal(valueOf('Style'), 'Custom');
    await openPage('Style');

    // A value rejected by the server is marked and not retried until edited.
    const saved = saves(fixture);
    fixture.failNext({
      kind: 'invalid',
      field: 'processing.custom_style.dark_floor_percent',
      message: 'must be 0-20',
    });
    await edit(knob('dark_floor_percent'), 12);
    await settle();
    assert.equal(
      errorOf(knob('dark_floor_percent')),
      'Prysm can’t use this value.',
    );
    await settle();
    assert.equal(saves(fixture), saved + 1);
    await edit(knob('dark_floor_percent'), 5);
    await settle();
    assert.equal(saves(fixture), saved + 2);
    assert.equal(
      fixture.snapshot().settings.processing.custom_style.dark_floor_percent,
      5,
    );
  } finally {
    await portal.unmount();
  }
});

test('the video source editor saves the exact section once; Cancel drops the draft', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { ...capture, fps: 25 },
    output,
  });
  const { fixture } = portal;
  const primary = () => button('Use this video source', form('Video source'));
  try {
    await openEditor('Video source');
    assert.equal(field('capture.device').value, '/dev/video0');
    assert.equal(radio('Custom').checked, true, 'no preset is 25 fps');
    // Nothing to save yet: no bar; the way out is the page's back link.
    assert(!primary(), 'no save bar');
    assert(!button('Cancel', form('Video source')), 'no Cancel');
    assert.equal(
      now(),
      'Not in use — Screen sync is off, so this source isn’t in use.',
    );

    await click(radio('Smooth'));
    assert.equal(primary().disabled, false);
    await click(button('Cancel', form('Video source')));
    assert.equal(window.location.hash, '#/settings');
    await openEditor('Video source');
    assert.equal(radio('Custom').checked, true, 'the draft was dropped');
    // A field edited back to its saved value leaves nothing to save.
    await edit(field('capture.device'), '/dev/video1');
    assert.equal(primary()?.disabled, false);
    await edit(field('capture.device'), '/dev/video0');
    assert(!primary(), 'the bar goes once the draft matches');

    await click(radio('Smooth'));
    assert.equal(saves(fixture), 0);
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings, {
      capture: {
        kind: 'device',
        device: '/dev/video0',
        fps: 60,
        width: 640,
        height: 360,
      },
      output,
      processing: defaultProcessing,
    });
    // Screen sync is off: saved, the bar gone, the status unchanged.
    assert(!primary(), 'the bar is gone');
    assert.equal(
      now(),
      'Not in use — Screen sync is off, so this source isn’t in use.',
    );

    // An empty device is reported once a save is tried, and never sent.
    await edit(field('capture.device'), ' ');
    await submit(form('Video source'));
    assert.equal(
      errorOf(field('capture.device')),
      'Choose a device, or enter its path.',
    );
    assert.equal(status(), 'Fix 1 field');
    assertFocused(field('capture.device'));
    assert.equal(saves(fixture), 1);
  } finally {
    await portal.unmount();
  }
});

test('server field errors land on their control and clear when it is edited', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await openEditor('Video source');
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.height',
      message: 'the device does not deliver 360 lines',
    });
    await click(radio('Smooth'));
    await submit(form('Video source'));
    // On the size-and-rate choice, which takes the focus; Prysm’s own words
    // stay out: it says how to fix it.
    const choice = $('.picture-choice');
    assert.equal(choice.getAttribute('aria-invalid'), 'true');
    assert.equal(
      hint(choice),
      'This device can’t capture this size. Choose another.',
    );
    assertFocused(choice);
    assert.equal(status(), 'Fix 1 field');
    await click(radio('Efficient'));
    assert(!choice.getAttribute('aria-invalid'));
    assert.equal(choice.getAttribute('aria-describedby'), null);

    await leaveEditor();
    await openEditor('Lights');
    fixture.failNext({
      kind: 'invalid',
      field: 'output.address',
      message: 'a unicast device address is required',
    });
    await edit(field('output.address'), '224.0.0.1');
    await submit(form('Your lights'));
    assert.equal(
      errorOf(field('output.address')),
      'Use the lights’ name, such as wled.local, or their number, such as 192.168.1.50.',
    );
    assertFocused(field('output.address'));

    fixture.failNext({
      kind: 'invalid',
      field: 'output.leds',
      message: 'the LEDs must total 1 to 16384, the most WLED drives',
    });
    await edit(field('output.address'), '192.0.2.7');
    await submit(form('Your lights'));
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'Use 1 to 16384 LEDs in total.',
    );
    assertFocused($('[data-field="output.leds"]'));
    // Editing any edge answers the group's error.
    await edit(field('output.leds.right'), 2);
    assert.equal($('[data-field-message="output.leds"]').textContent, '');

    // A field this form does not show still explains itself.
    fixture.failNext({
      kind: 'invalid',
      field: 'output.unknown',
      message: 'unknown field',
    });
    await submit(form('Your lights'));
    assertFocused($('[data-form-alert]', form('Your lights')));
    assert.equal(
      document.activeElement.textContent,
      'Not saved: Prysm can’t use these settings.',
    );
    // Every attempt reached the server and was refused; nothing was saved.
    assert.equal(saves(fixture), 4);
    assert.equal(fixture.snapshot().revision, 0);
  } finally {
    await portal.unmount();
  }
});

test('a saved section without a field its kind requires opens its editor with that field empty', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'device', path: '/dev/video0' },
    output,
  });
  const { fixture } = portal;
  try {
    // Through Home and Settings, which show it as well.
    await openEditor('Video source');
    assert.equal(field('capture.device').value, '');
    await edit(field('capture.device'), '/dev/video0');
    await submit(form('Video source'));
    // The rest as saved, which Prysm refuses by name.
    assert.equal(saves(fixture), 1);
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'device',
      path: '/dev/video0',
      device: '/dev/video0',
    });
  } finally {
    await portal.unmount();
  }
});

test('a saved kind named like an object property is a kind without a panel', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'constructor' },
    output,
  });
  try {
    assert(!$$('h2').some((h2) => h2.textContent === 'Something went wrong'));
  } finally {
    await portal.unmount();
  }
});

test('a section changed on another device: use theirs, or keep mine', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const conflict =
    'The video source was changed on another device since you opened this.';
  try {
    await openEditor('Video source');
    // Seen before saving: the draft is kept, and saving asks.
    await edit(field('capture.device'), '/dev/video1');
    await elsewhere(fixture, {
      capture: { kind: 'device', device: '/dev/video9' },
    });
    assert.equal(field('capture.device').value, '/dev/video1');
    await submit(form('Video source'));
    assert.equal(saves(fixture), 1, 'only the other device saved');
    assert.equal(status(), conflict);
    await click(button('Use theirs'));
    assert.equal(field('capture.device').value, '/dev/video9');

    // Refused by Prysm (409): the same choice; keeping mine saves it.
    await edit(field('capture.device'), '/dev/video2');
    fixture.failNext({
      kind: 'stale_revision',
      message: 'settings revision 1 is stale; current is 2',
      current: 2,
    });
    await submit(form('Video source'));
    assert.equal(status(), conflict);
    assert.equal(field('capture.device').value, '/dev/video2');
    await click(button('Keep mine'));
    assert.equal(fixture.snapshot().settings.capture.device, '/dev/video2');

    // An editor without changes follows other devices.
    await leaveEditor();
    await openEditor('Lights');
    await elsewhere(fixture, { output: { ...output, address: '192.0.2.50' } });
    assert.equal(field('output.address').value, '192.0.2.50');

    // Another section saved meanwhile is no conflict.
    await edit(field('output.address'), '192.0.2.60');
    await elsewhere(fixture, {
      processing: {
        ...fixture.snapshot().settings.processing,
        brightness_percent: 50,
      },
    });
    await submit(form('Your lights'));
    assert.equal(fixture.snapshot().settings.output.address, '192.0.2.60');
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 50);
    assert.equal(fixture.snapshot().settings.capture.device, '/dev/video2');
  } finally {
    await portal.unmount();
  }
});

/** The live problem line of a part of the setup, on Settings. */
const problem = (label) => $('.row-problem', row(label))?.textContent ?? '';

test('the status follows saved lights; failing ones can be swapped back', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await press(mode('screen_sync'));
    await openSettings();
    assert.equal(problem('Lights'), '', 'nothing wrong: an empty line, kept');
    await openEditor('Lights');
    assert.equal(now(), 'Starting… — WLED at 192.0.2.1 · 14 LEDs');
    assert(!button('Go back to the previous lights'), 'nothing to go back to');
    await edit(field('output.address'), '192.0.2.99');
    await submit(form('Your lights'));
    assert.equal(fixture.snapshot().settings.output.address, '192.0.2.99');
    // A plain save: the bar leaves, the status follows the new lights.
    assert(!button('Use these lights'));
    assert.equal(now(), 'Starting… — WLED at 192.0.2.99 · 14 LEDs');
    assert(!button('Go back to the previous lights'), 'only when needed');

    // The new address does not answer: said plainly, with the way back.
    await act(async () =>
      fixture.publish({
        session: {
          state: 'stopped',
          failure: {
            kind: 'adapter',
            section: 'output',
            message: 'no answer from 192.0.2.99',
          },
        },
      }),
    );
    assert.equal(now(), 'Lights not responding — WLED at 192.0.2.99 · 14 LEDs');
    // Saved here: the bar, where the user is, says why in Prysm's words.
    assert.equal(status(), 'Lights not responding: no answer from 192.0.2.99.');
    assert(button('Go back to the previous lights', $('.page-actions')));
    assert(!$('.section-status-more'), 'said once, in the bar');

    // Settings shows the live problem on its row; the way back stays.
    await leaveEditor();
    assert.equal(problem('Lights'), 'The lights aren’t responding.');
    // The device stays named, before the problem.
    assert.equal(valueOf('Lights'), 'WLED · The lights aren’t responding.');
    assert.equal(problem('Video source'), '');
    await openEditor('Lights');
    await click(button('Go back to the previous lights'));
    assert.deepEqual(fixture.snapshot().settings.output, output);
    assert.equal(field('output.address').value, '192.0.2.1');
    await act(async () =>
      fixture.publish({
        session: { state: 'screen_sync' },
        input: { fps: 30, receiving: true },
      }),
    );
    assert.equal(now(), 'Showing screen sync — WLED at 192.0.2.1 · 14 LEDs');
    assert(!button('Go back to the previous lights'), 'used up');
    await leaveEditor();
    assert.equal(problem('Lights'), '');

    // The video source: the picture's rate while it arrives.
    await openEditor('Video source');
    assert.equal(now(), 'Picture arriving — 30 fps');
    await edit(field('capture.device'), '/dev/video3');
    await submit(form('Video source'));
    assert(!button('Use this video source'));
    assert(!button('Go back to the previous video source'));
  } finally {
    await portal.unmount();
  }
});

test('a source that sends nothing for a while is a problem, with the way back', async (t) => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await press(mode('screen_sync'));
    await openEditor('Video source');
    await edit(field('capture.device'), '/dev/video4');
    await submit(form('Video source'));
    // The session restarts for the new source: stopped, then running.
    await act(async () => fixture.publish({ session: { state: 'stopped' } }));
    t.mock.timers.enable({ apis: ['setTimeout'] });
    await act(async () =>
      fixture.publish({
        session: { state: 'screen_sync' },
        input: { fps: 0, receiving: false },
      }),
    );
    assert.equal(now(), 'Starting… — Waiting for the picture.');
    assert(!button('Go back to the previous video source'));
    await act(async () => t.mock.timers.tick(checkForMs));
    t.mock.timers.reset();
    assert.equal(now(), 'No picture — ');
    assert.equal(
      status(),
      'No picture: Nothing has arrived from this source.',
      'the reason is said once, in the bar',
    );
    await click(button('Go back to the previous video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, capture);
  } finally {
    await portal.unmount();
  }
});

test('after a save the bar stays where the user is: starting, working, then gone; or why it fails', async (t) => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const bar = () => $('.page-actions', form('Video source'));
  try {
    await press(mode('screen_sync'));
    await act(async () =>
      fixture.publish({
        session: { state: 'screen_sync' },
        input: { fps: 30, receiving: true },
      }),
    );
    await openEditor('Video source');
    assert.equal(bar().hidden, true, 'nothing to save: no bar');
    await edit(field('capture.device'), '/dev/video5');
    assert.equal(status(), 'Saving restarts screen sync.');
    await submit(form('Video source'));
    await act(async () => fixture.publish({ session: { state: 'stopped' } }));
    await act(async () =>
      fixture.publish({
        session: { state: 'screen_sync' },
        input: { fps: 0, receiving: false },
      }),
    );
    assert.equal(status(), 'Saved. Starting…');
    assert.equal(bar().hidden, false);
    assert(!button('Use this video source'), 'no button to press again');
    t.mock.timers.enable({ apis: ['setTimeout'] });
    await act(async () =>
      fixture.publish({ input: { fps: 30, receiving: true } }),
    );
    assert.equal(status(), 'Saved. Picture arriving.');
    await act(async () => t.mock.timers.tick(outcomeForMs));
    t.mock.timers.reset();
    assert.equal(bar().hidden, true, 'working: the bar goes');
    assert.equal(now(), 'Picture arriving — 30 fps', 'the card stays');

    // A save that fails: Prysm's reason and the way back, in the bar.
    await edit(field('capture.device'), '/dev/video6');
    await submit(form('Video source'));
    await act(async () =>
      fixture.publish({
        session: {
          state: 'stopped',
          failure: {
            kind: 'adapter',
            section: 'capture',
            message: 'cannot open /dev/video6',
          },
        },
      }),
    );
    assert.equal(status(), 'No picture: cannot open /dev/video6.');
    assert.equal($('.form-status').dataset.tone, 'error');
    assert(!$('.section-status-more'), 'not said twice');
    await click(button('Go back to the previous video source', bar()));
    assert.equal(fixture.snapshot().settings.capture.device, '/dev/video5');
  } finally {
    await portal.unmount();
  }
});

test('the save bar says how many fields to fix and takes the user to the first', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'remote' },
    output,
  });
  const { fixture } = portal;
  const advanced = () => button('Advanced', form('Video source'));
  try {
    await openEditor('Video source');
    // TV-only options wait under Advanced, closed.
    assert.equal(advanced().getAttribute('aria-expanded'), 'false');
    assert(field('capture.encoding').closest('[hidden]'));
    // A problem inside Advanced opens it, and the jump lands there.
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.encoding',
      message: 'the sender cannot stream h264_software',
    });
    await edit(field('capture.encoding'), 'h264_software');
    await submit(form('Video source'));
    assert.equal(saves(fixture), 1, 'asked once, refused');
    assert.equal(advanced().getAttribute('aria-expanded'), 'true');
    assertFocused(field('capture.encoding'));
    assert.equal(status(), 'Fix 1 field');
    field('capture.device')?.focus();
    await click(button('Fix 1 field'));
    assertFocused(field('capture.encoding'));
  } finally {
    await portal.unmount();
  }
});

test('the status never opens the source: off, it says so and shows no picture', async () => {
  const preview = fakePreviewClient();
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    { preview: preview.client },
  );
  const { fixture } = portal;
  try {
    await openEditor('Video source');
    assert.equal(
      now(),
      'Not in use — Screen sync is off, so this source isn’t in use.',
    );
    assert.equal(preview.opened(), 0, 'no preview stream while off');
    await edit(field('capture.device'), '/dev/video4');
    await submit(form('Video source'));
    assert.equal(fixture.snapshot().target, 'off', 'saving starts nothing');
    assert.equal(preview.opened(), 0);
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
        input: { fps: 29.9, receiving: true },
      }),
    );
    assert.equal(preview.opened(), 1, 'watched while the picture flows');
    assert.equal(now(), 'Picture arriving — 30 fps');
  } finally {
    await portal.unmount();
  }
});

test('a video file and no lights are developer options; a saved one still shows', async () => {
  const saved = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'file', path: '/clip.mkv' },
    output: { kind: 'null' },
  });
  try {
    // Saved setups of such kinds show as they are, and stay editable.
    await openEditor('Video source');
    assert.deepEqual(kinds('capture.kind'), ['device', 'remote', 'file']);
    assert.equal(field('capture.path').value, '/clip.mkv');
    await leaveEditor();
    await openEditor('Lights');
    assert.deepEqual(kinds('output.kind'), ['wled', 'null']);
  } finally {
    await saved.unmount();
  }

  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    await openEditor('Video source');
    assert.deepEqual(kinds('capture.kind'), ['device', 'remote']);
    await leaveEditor();
    // WLED is then the only lights: no choice to make.
    await openEditor('Lights');
    assert(!kindRows('output.kind'));
    await leaveEditor();
    // On in About this Prysm, for this browser.
    await openPage('About this Prysm');
    assert.equal(field('developer').checked, false);
    await click(field('developer'));
    assert.equal(field('developer').checked, true);
    await click(button('Settings'));
    await openEditor('Lights');
    assert.deepEqual(kinds('output.kind'), ['wled', 'null']);
  } finally {
    await portal.unmount();
  }
});

test('kind rows swap panels without losing drafts', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const remote = {
    kind: 'remote',
    width: 320,
    height: 180,
    fps: 30,
    include_ui: true,
    encoding: 'h264_hardware',
  };
  try {
    // A video file and no lights are developer options.
    await act(async () => saveDeveloper(true));
    await openEditor('Video source');
    // A choice in the rows pattern, named by its heading alone.
    const group = kindRows('capture.kind');
    assert.equal(group.getAttribute('role'), 'radiogroup');
    assert.equal(
      document.getElementById(group.getAttribute('aria-labelledby'))
        .textContent,
      'Capture from',
    );
    assert.deepEqual(
      $$('input', kindRows('capture.kind')).map((input) => [
        input.value,
        input.closest('label').textContent,
      ]),
      [
        ['device', 'Capture card or camera'],
        ['remote', 'LG TV on your network'],
        ['file', 'Video file (development only)'],
      ],
    );
    await chooseKind('capture.kind', 'remote');
    assert(!field('capture.device'));
    // The TV reaches this computer at the portal's address: no address field.
    assert(!field('capture.listen'));
    await chooseKind('capture.kind', 'device');
    assert.equal(field('capture.device').value, '/dev/video0');
    await chooseKind('capture.kind', 'file');
    assert.equal(field('capture.path').value, '');
    assert.equal(field('capture.path').labels[0].textContent, 'Video file');
    // Development only: it says what it needs.
    assert.match(
      form('Video source').textContent,
      /It plays once, through FFmpeg 9 or newer\./,
    );
    await chooseKind('capture.kind', 'remote');
    await submit(form('Video source'));
    // Written in full: a later default changes only new setups.
    assert.deepEqual(fixture.snapshot().settings.capture, remote);
    assert.deepEqual(fixture.snapshot().settings.output, output);

    await leaveEditor();
    await openEditor('Lights');
    assert.deepEqual(kinds('output.kind'), ['wled', 'null']);
    await edit(field('output.address'), 'wled.local');
    await chooseKind('output.kind', 'null');
    assert(
      form('Your lights').textContent.includes(
        'There is nothing to configure.',
      ),
    );
    await chooseKind('output.kind', 'wled');
    assert.equal(field('output.address').value, 'wled.local', 'kept');
    await chooseKind('output.kind', 'null');
    await submit(form('Your lights'));
    assert.deepEqual(fixture.snapshot().settings, {
      capture: remote,
      output: { kind: 'null' },
      processing: defaultProcessing,
    });
    // Home: no WLED link, and these lights have no effects of their own.
    await leaveEditor();
    await click(button('Home'));
    assert(!$('.device-link'));
    assert.equal(mode('effects').disabled, true);
  } finally {
    await portal.unmount();
  }
});

test('an unconfigured section starts from its first kind and saves only when valid', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {});
  const { fixture } = portal;
  try {
    await click(button('Leave setup'));
    await openSettings();
    assert.equal(valueOf('Video source'), 'Not set up yet');
    await openEditor('Video source');
    assert.equal(kindOf('capture.kind'), 'device');
    assert.equal(errorOf(field('capture.device')), undefined);
    // An unconfigured section offers its save before any edit.
    assert(button('Use this video source', form('Video source')));
    await submit(form('Video source'));
    assert.equal(
      errorOf(field('capture.device')),
      'Choose a device, or enter its path.',
    );
    assert.equal(saves(fixture), 0);
    await edit(field('capture.device'), ' /dev/video0 ');
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'device',
      device: '/dev/video0',
      width: 640,
      height: 360,
      fps: 30,
    });
    // Lights still missing: Home keeps offering setup.
    await leaveEditor();
    await click(button('Home'));
    assert.equal(button('Set up Prysm').disabled, false);
  } finally {
    await portal.unmount();
  }
});

test('the WLED panel edits edge counts and shows the wiring as the renderer orders it', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const edges = ['top', 'right', 'bottom', 'left'];
  const caption = () => $('.led-layout figcaption').textContent;
  try {
    await openEditor('Lights');
    assert.deepEqual(
      edges.map((edge) => field(`output.leds.${edge}`).value),
      ['10', '0', '0', '4'],
    );
    assert.deepEqual(
      [
        field('output.leds.top').min,
        field('output.leds.top').max,
        field('output.leds.top').step,
      ],
      ['0', '16384', '1'],
    );
    assert.equal(
      caption(),
      'LED 1 is at the bottom-left corner. Order: left upward: LEDs 1–4; top rightward: LEDs 5–14; right: none; bottom: none.',
    );
    // The total in the drawing's middle; the maximum only when exceeded.
    assert.equal($('.screen-label').textContent, '14 LEDs in total');
    assert(!text().includes('16384'));

    for (const edge of edges) await edit(field(`output.leds.${edge}`), 0);
    assert.equal(caption(), 'No LEDs yet.');
    await submit(form('Your lights'));
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'Use 1 to 16384 LEDs in total.',
    );
    assertFocused($('[data-field="output.leds"]'));
    await edit(field('output.leds.top'), 20000);
    assert.equal(
      errorOf(field('output.leds.top')),
      'Enter a whole number from 0 to 16384.',
    );
    await edit(field('output.leds.top'), 16384);
    await edit(field('output.leds.left'), 1);
    assert(text().includes('Use 1 to 16384 LEDs in total.'));
    assert.equal(saves(fixture), 0);

    for (const [edge, count] of [
      ['top', 96],
      ['right', 54],
      ['bottom', 96],
      ['left', 0],
    ])
      await edit(field(`output.leds.${edge}`), count);
    assert.equal(
      caption(),
      'LED 1 is at the top-left corner. Order: left: none; top rightward: LEDs 1–96; right downward: LEDs 97–150; bottom leftward: LEDs 151–246.',
    );
    await edit(field('output.address'), ' http://wled.local:8080/ ');
    await submit(form('Your lights'));
    assert.deepEqual(fixture.snapshot().settings.output, {
      kind: 'wled',
      address: 'wled.local:8080',
      leds: { top: 96, right: 54, bottom: 96, left: 0 },
    });
    assert.equal(field('output.address').value, 'wled.local:8080');
    await leaveEditor();
    await click(button('Home'));
    // The way to WLED, with its effects chosen.
    await press(mode('effects'));
    assert.equal($('.light-calm .device-link').href, 'http://wled.local:8080/');
  } finally {
    await portal.unmount();
  }
});

test('smooth dark colours waits under Advanced in the WLED panel, on unless turned off', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const advanced = () => button('Advanced', form('Your lights'));
  const smooth = () => field('output.dither');
  try {
    await openEditor('Lights');
    assert.equal(advanced().getAttribute('aria-expanded'), 'false');
    assert(smooth().closest('[hidden]'));
    assert.equal(smooth().checked, true, 'on while the settings leave it out');
    await click(advanced());
    assert(!smooth().closest('[hidden]'));
    await click(smooth());
    await submit(form('Your lights'));
    assert.deepEqual(fixture.snapshot().settings.output, {
      ...output,
      dither: false,
    });
  } finally {
    await portal.unmount();
  }
});

test('the WLED panel lists the lights present as rows, then one to type in, which holds the saved address while it is not found', async () => {
  const discovery = fakeDiscoveryClient({
    'output/wled': [
      {
        label: 'Living room',
        detail: '192.0.2.40',
        fields: { address: '192.0.2.40' },
      },
    ],
  });
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    { discovery: discovery.client },
  );
  const { fixture } = portal;
  /** Each row: its name (the typed one: its text), its line, ticked. */
  const lights = () =>
    $$('.device-rows .row-choice').map((row) => [
      $('.row-label', row)?.textContent ?? $('input[type="text"]', row).value,
      $('.row-value', row)?.textContent,
      $('input', row).checked,
    ]);
  try {
    await openEditor('Lights');
    await act(async () => sleep(0));
    // Looked for as the panel opened; the saved address in the last row,
    // ticked, with nothing said against it: lights can be there and not
    // found. Advanced keeps only smooth dark colours.
    assert.deepEqual(lights(), [
      ['Living room', undefined, false],
      [output.address, undefined, true],
    ]);
    assert(!field('output.address').closest('.advanced'));
    assert.equal(
      $('.row-typed input[type="radio"]').getAttribute('aria-label'),
      'Other lights',
    );
    // Picking found lights keeps what was typed, to pick it again.
    await click($('.device-rows input'));
    assert.deepEqual(lights(), [
      ['Living room', undefined, true],
      [output.address, undefined, false],
    ]);
    assert.equal(field('output.address').tabIndex, -1, 'tabbed past');
    await click($('.row-typed input'));
    assert.deepEqual(lights(), [
      ['Living room', undefined, false],
      [output.address, undefined, true],
    ]);
    await click($('.device-rows input'));
    await submit(form('Your lights'));
    assert.equal(fixture.snapshot().settings.output.address, '192.0.2.40');
  } finally {
    await portal.unmount();
  }
});

test('with lights to pick, no address is said at the typed row, which takes the focus; Advanced stays closed', async () => {
  const discovery = fakeDiscoveryClient({
    'output/wled': [
      {
        label: 'Living room',
        detail: '192.0.2.40',
        fields: { address: '192.0.2.40' },
      },
    ],
  });
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    { discovery: discovery.client },
  );
  const { fixture } = portal;
  try {
    await openEditor('Lights');
    await act(async () => sleep(0));
    await edit(field('output.address'), '');
    await submit(form('Your lights'));
    assertFocused(field('output.address'));
    assert.equal(
      errorOf(field('output.address')),
      'Choose your lights, or enter their address.',
    );
    assert.equal($('.device-rows').getAttribute('aria-invalid'), null);
    assert.equal(
      button('Advanced', form('Your lights')).getAttribute('aria-expanded'),
      'false',
    );
    assert.equal(fixture.snapshot().settings.output.address, output.address);
  } finally {
    await portal.unmount();
  }
});

test('with nothing found, a refused typed address is said once, at the field, which takes the focus', async () => {
  const discovery = fakeDiscoveryClient();
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    { discovery: discovery.client },
  );
  const { fixture } = portal;
  const refused =
    'Use the lights’ name, such as wled.local, or their number, such as 192.168.1.50.';
  try {
    await openEditor('Lights');
    await act(async () => sleep(0));
    // The saved lights are in the row to type in, the only row.
    assert.equal($$('.device-rows .row-choice').length, 1);
    assert.equal(field('output.address').value, output.address);
    fixture.failNext({
      kind: 'invalid',
      field: 'output.address',
      message: 'unsupported scheme',
    });
    await edit(field('output.address'), 'https://wled');
    await submit(form('Your lights'));
    assertFocused(field('output.address'));
    assert.equal(errorOf(field('output.address')), refused);
    const group = $('.device-rows');
    assert.equal(group.dataset.field, undefined, 'the field owns its path');
    assert.equal(group.getAttribute('aria-invalid'), null);
    assert.equal(
      $('[role="status"]', group).textContent,
      'No WLED lights found on this network. Enter their address.',
    );
    assert.equal(text().split(refused).length, 2, 'said once');
  } finally {
    await portal.unmount();
  }
});

test('each edge’s LED count is typed on that edge of the drawing', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    await openEditor('Lights');
    const drawing = $('.led-layout');
    for (const [edge, label] of [
      ['top', 'Top LEDs'],
      ['right', 'Right LEDs'],
      ['bottom', 'Bottom LEDs'],
      ['left', 'Left LEDs'],
    ]) {
      const input = $(`.led-edge-${edge} input`, drawing);
      assertSame(input, field(`output.leds.${edge}`), `${edge} on its edge`);
      assert.equal(input.labels[0].textContent, label);
    }
    assert(!$('.led-counts'), 'no second set of fields');
    await edit(field('output.leds.right'), 6);
    assert.equal($('.screen-label').textContent, '20 LEDs in total');
    // Too many: said at once, the total marked.
    await edit(field('output.leds.bottom'), 16384);
    assert.equal($('.screen-label').dataset.invalid, 'true');
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'Use 1 to 16384 LEDs in total.',
    );
    await edit(field('output.leds.bottom'), 0);
    assert.equal($('.screen-label').dataset.invalid, 'false');
    // An edge's own problem shows under the drawing too.
    await edit(field('output.leds.top'), 20000);
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'Enter a whole number from 0 to 16384.',
    );
  } finally {
    await portal.unmount();
  }
});

test('the drawing badges LED 1 and says once where it is and which way the strip runs', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  // The sentence after the line's badge.
  const way = () => $('.led-way').lastChild.textContent;
  const start = () => $('.led-start');
  try {
    await openEditor('Lights');
    // The first lit edge decides: left, else top, else right, else bottom.
    for (const [edges, corner, along] of [
      [{}, 'bottom-left', 'up'],
      [{ left: 0 }, 'top-left', 'right'],
      [{ right: 5, top: 0 }, 'top-right', 'down'],
      [{ bottom: 5, right: 0 }, 'bottom-right', 'left'],
    ]) {
      for (const [edge, count] of Object.entries(edges))
        await edit(field(`output.leds.${edge}`), count);
      assert.equal(
        way(),
        `First LED is ${corner.replace('-', ' ')}. The strip runs clockwise.`,
      );
      assert.equal(start().dataset.corner, corner);
      assert.equal(start().dataset.way, along);
      assert.equal($('.led-edge[data-way]').dataset.way, along, 'one mark');
    }
    await edit(field('output.leds.bottom'), 0);
    assert.equal(way(), 'No LEDs yet. Enter how many are on each edge.');
    assert(!start(), 'no badge without LEDs');
    assert(!$('.led-edge[data-way]'));
  } finally {
    await portal.unmount();
  }
});

test('LED runs follow render_edges: bottom-left, clockwise, empty edges skipped', () => {
  assert.deepEqual(
    ledRuns({ top: 3, right: 2, bottom: 3, left: 2 }).map(
      ({ edge, first, last, from }) => [edge, first, last, from],
    ),
    [
      ['left', 1, 2, 'bottom-left'],
      ['top', 3, 5, 'top-left'],
      ['right', 6, 7, 'top-right'],
      ['bottom', 8, 10, 'bottom-right'],
    ],
  );
});

test('fields the forms do not show are preserved on every save', async () => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { ...capture, future_option: 1 }, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        colour: { calibration: 'warm' },
        processing: {
          ...snapshot.settings.processing,
          active_calibration: 'ps5',
          calibrations: [{ id: 'ps5', name: 'PS5', correction: noCorrection }],
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    // A video file and no lights are developer options.
    await act(async () => saveDeveloper(true));
    await openEditor('Video source');
    await click(radio('Smooth'));
    await submit(form('Video source'));
    await leaveEditor();
    await openEditor('Lights');
    await edit(field('output.leds.right'), 6);
    await submit(form('Your lights'));
    await leaveEditor();
    await openPage('Detail');
    await edit(field('processing.sample_density'), 20);
    await settle();
    assert.deepEqual(fixture.snapshot().settings, {
      capture: {
        ...capture,
        future_option: 1,
        width: 640,
        height: 360,
        fps: 60,
      },
      output: { ...output, leds: { ...output.leds, right: 6 } },
      processing: {
        ...defaultProcessing,
        sample_density: 20,
        active_calibration: 'ps5',
        calibrations: [{ id: 'ps5', name: 'PS5', correction: noCorrection }],
      },
      colour: { calibration: 'warm' },
    });
  } finally {
    await portal.unmount();
  }
});

/** The size-and-rate choice in the open editor: its rows and the ticked one. */
const presetRows = () =>
  $$('input[type="radio"]', $('.picture-choice', form('Video source')));
const presetLabel = (input) =>
  document.getElementById(input.getAttribute('aria-labelledby')).textContent;
const ticked = () =>
  presetRows()
    .filter((input) => input.checked)
    .map(presetLabel);

test('the saved size and rate tick their preset, omitted fields at their defaults; anything else shows as Custom', async () => {
  const device = '/dev/video0';
  const presets = ['Balanced', 'Smooth', 'Efficient'];
  const custom = [...presets, 'Custom'];
  for (const [Host, saved, rows, expected] of [
    [WebosWebPortal, { kind: 'webos' }, presets, 'Balanced'],
    [WebosWebPortal, { kind: 'webos', fps: 60 }, presets, 'Smooth'],
    [
      WebosWebPortal,
      { kind: 'webos', width: 320, height: 180 },
      presets,
      'Efficient',
    ],
    [
      WebosWebPortal,
      { kind: 'webos', width: 640, height: 360, fps: 30 },
      presets,
      'Balanced',
    ],
    [WebosWebPortal, { kind: 'webos', fps: 25 }, custom, 'Custom'],
    [
      WebosWebPortal,
      { kind: 'webos', width: 1280, height: 720 },
      custom,
      'Custom',
    ],
    // A capture-only TV's defaults are 320 × 180 at 30 fps.
    [HeadlessWebPortal, { kind: 'remote' }, presets, 'Efficient'],
    [
      HeadlessWebPortal,
      { kind: 'remote', width: 640, height: 360 },
      presets,
      'Balanced',
    ],
    [HeadlessWebPortal, { kind: 'device', device }, presets, 'Balanced'],
    [HeadlessWebPortal, { kind: 'device', device, fps: 60 }, presets, 'Smooth'],
    [
      HeadlessWebPortal,
      { kind: 'file', path: '/clip.mkv', width: 320, height: 180 },
      presets,
      'Efficient',
    ],
  ]) {
    const portal = await renderPortal(Host, { capture: saved, output });
    const name = JSON.stringify(saved);
    try {
      await act(async () => saveDeveloper(true));
      await openEditor('Video source');
      assert.deepEqual(presetRows().map(presetLabel), rows, name);
      assert.deepEqual(ticked(), [expected], name);
      assert(!field('capture.width'), 'no number fields');
      assert(!button('Use this video source'), 'nothing moved');
    } finally {
      await portal.unmount();
    }
  }
});

test('the size-and-rate choice is a labelled radio group whose rows describe their trade and numbers', async () => {
  const portal = await renderPortal(WebosWebPortal, {
    capture: { kind: 'webos' },
    output,
  });
  const described = (input) =>
    (input.getAttribute('aria-describedby') ?? '')
      .split(' ')
      .filter(Boolean)
      .map((id) =>
        document.getElementById(id).textContent.replace(/\u00a0/g, ' '),
      );
  try {
    await openEditor('Video source');
    const group = $('[role="radiogroup"]', form('Video source'));
    assert.equal(
      document.getElementById(group.getAttribute('aria-labelledby'))
        .textContent,
      'Size and rate',
    );
    const rows = presetRows();
    assert.equal(new Set(rows.map((input) => input.name)).size, 1, 'one group');
    // One line each; only the ticked row says what it means.
    assert.deepEqual(rows.map(described), [
      ['Right for most setups.', '640 × 360 · 30 fps'],
      ['640 × 360 · 60 fps'],
      ['320 × 180 · 30 fps'],
    ]);
  } finally {
    await portal.unmount();
  }
});

test('choosing a preset saves exactly its size and rate; a refused one is said at the choice', async () => {
  const portal = await renderPortal(WebosWebPortal, {
    capture: { kind: 'webos', include_ui: false },
    output,
  });
  const { fixture } = portal;
  const saved = () => fixture.snapshot().settings.capture;
  try {
    await openEditor('Video source');
    for (const [label, width, height, fps] of [
      ['Smooth', 640, 360, 60],
      ['Efficient', 320, 180, 30],
      ['Balanced', 640, 360, 30],
    ]) {
      await click(radio(label));
      assert.deepEqual(ticked(), [label]);
      await submit(form('Video source'));
      assert.deepEqual(saved(), {
        kind: 'webos',
        include_ui: false,
        width,
        height,
        fps,
      });
    }

    // A refused size is said above the rows, which take the focus.
    await click(radio('Efficient'));
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.width',
      message: 'unsupported size',
    });
    await submit(form('Video source'));
    const choice = $('.picture-choice');
    assert.deepEqual(ticked(), ['Efficient']);
    assert.equal(choice.getAttribute('aria-invalid'), 'true');
    assert.equal(hint(choice), 'Prysm can’t use this value.');
    assertFocused(choice);
    await click(radio('Balanced'));
    assert.equal(choice.getAttribute('aria-invalid'), null);
  } finally {
    await portal.unmount();
  }
});

test('the webOS portal edits TV capture with its presets and menus', async () => {
  const portal = await renderPortal(WebosWebPortal, {
    capture: { kind: 'webos' },
    output,
  });
  const { fixture } = portal;
  try {
    await openEditor('Video source');
    assert(!kindRows('capture.kind'));
    assert.equal($('.page-heading h2').textContent, 'Video source');
    assert.equal(field('capture.include_ui').checked, true);
    await click(radio('Smooth'));
    await click(field('capture.include_ui'));
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'webos',
      width: 640,
      height: 360,
      fps: 60,
      include_ui: false,
    });
    assert.deepEqual(fixture.snapshot().settings.output, output);
    await leaveEditor();
    await openEditor('Lights');
    assert(!kindRows('output.kind'));
  } finally {
    await portal.unmount();
  }
});

test('the remote capture form asks the TV for a stream with the same picture choice as the webOS form', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'remote' },
    output,
  });
  const { fixture } = portal;
  const input = (name) => field(`capture.${name}`);
  const help = (element) =>
    element.closest('.field').querySelector('.help')?.textContent;
  try {
    await openEditor('Video source');
    // A capture-only TV's default, 320 × 180 at 30 fps, is Efficient.
    assert.equal(radio('Efficient').checked, true);
    assert.deepEqual(
      ['include_ui', 'encoding'].map(
        (name) => input(name).labels[0].textContent,
      ),
      ['Include TV menus', 'How the TV sends the picture'],
    );
    assert.equal(input('include_ui').checked, true);
    assert.equal(help(input('include_ui')), includeMenusHelp);
    assert.match(includeMenusHelp, /over the video/);
    assert.match(includeMenusHelp, /nothing is playing/);
    assert.deepEqual(
      $$('option', input('encoding')).map((item) => [
        item.value,
        item.textContent,
      ]),
      [
        ['h264_hardware', 'Compressed by the TV’s video chip (recommended)'],
        ['h264_software', 'Compressed by the TV’s processor'],
        ['raw', 'Uncompressed'],
      ],
    );
    assert.match(
      help(input('encoding')),
      /Easiest for the TV and light on your network/,
    );
    await edit(input('encoding'), 'h264_software');
    assert.match(
      help(input('encoding')),
      /More work for the TV, equally light on your network/,
    );
    await edit(input('encoding'), 'raw');
    assert.match(help(input('encoding')), /ten times the network/);
    await click(radio('Smooth'));
    await click(input('include_ui'));
    await edit(input('encoding'), 'h264_software');
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'remote',
      width: 640,
      height: 360,
      fps: 60,
      include_ui: false,
      encoding: 'h264_software',
    });
    assert.match(
      status(),
      /^Saved\. Screen sync is off/,
      'saved, said in the bar',
    );

    // The server's own check on the encoding lands on the selector.
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.encoding',
      message: 'the sender cannot stream h264_hardware',
    });
    await edit(input('encoding'), 'h264_hardware');
    await submit(form('Video source'));
    assert.equal(
      errorOf(input('encoding')),
      'This TV can’t send the picture this way. Choose another.',
    );
    assertFocused(input('encoding'));
  } finally {
    await portal.unmount();
  }
});

test('the sender refusing the requested stream is shown in full with the lighting state', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'remote' },
    output,
  });
  const { fixture } = portal;
  const refusal =
    'the sender cannot stream 320x180 at 30 fps with menus as h264_hardware: cannot encode frame: hardware H.264 encoder cannot start: the TV video encoder is in use by another application (code -12)';
  try {
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: {
          state: 'stopped',
          failure: { kind: 'adapter', message: refusal },
        },
      }),
    );
    assert.equal($('.lighting-status').textContent, 'Screen sync stopped');
    assert.equal(
      $('.light-card [role="alert"]').textContent,
      'Screen sync couldn’t start.',
    );
    assert.equal($('.light-details p').textContent, refusal, 'not shortened');
    // The video source editor says where the fix is.
    await openEditor('Video source');
    assert.equal(
      $('.section-status-more').textContent,
      `${refusal}If the TV can’t send its picture, try another way under Advanced.`,
      'Prysm’s words in full, then what to change here',
    );
  } finally {
    await portal.unmount();
  }
});

test('limits written in the panels match the generated schemas', async () => {
  const schemas = async (name) =>
    JSON.parse(
      await readFile(
        new URL(
          `../packages/contracts/generated/${name}.openapi.json`,
          import.meta.url,
        ),
        'utf8',
      ),
    ).components.schemas;
  const application = await schemas('application');
  const desktop = await schemas('host-desktop');
  const webos = await schemas('host-webos');
  const edges = Object.fromEntries(
    ['top', 'right', 'bottom', 'left'].map((edge) => [edge, ledEdgeLimits]),
  );
  for (const [schema, limits] of [
    [application.Processing, processingLimits],
    [application.StyleValues, styleLimits],
    [application.CorrectionControls, correctionLimits],
    [application.PrimaryControls, primaryLimits],
    [desktop.LedCounts, edges],
    [webos.LedCounts, edges],
  ]) {
    for (const [name, { min, max }] of Object.entries(limits)) {
      const property = schema.properties[name];
      assert.equal(min, property.minimum, name);
      // Where the schema has no maximum, the panel documents the type bound it uses.
      if (property.maximum !== undefined)
        assert.equal(max, property.maximum, name);
    }
  }
  assert.equal(
    calibrationLimits.count,
    application.Processing.properties.calibrations.maxItems,
  );
  assert.equal(
    calibrationLimits.name,
    application.Calibration.properties.name.maxLength,
  );
  for (const name of Object.keys(
    application.CorrectionControls.properties,
  ).filter((name) => !['red', 'green', 'blue'].includes(name)))
    assert(name in correctionLimits, `${name} has a control`);
  for (const name of Object.keys(application.PrimaryControls.properties))
    assert(name in primaryLimits, `${name} has a control`);
  for (const name of Object.keys(application.StyleValues.properties))
    assert(name in styleLimits, `${name} has a slider`);
  // Every style has words, in the order Rust offers them, Custom last; the
  // built-in ones have generated values within the sliders' limits.
  assert.deepEqual(styleOrder, application.Style.enum);
  assert.deepEqual(
    Object.keys(styleValues).sort(),
    styleOrder.filter((style) => style !== 'custom').sort(),
  );
  for (const values of Object.values(styleValues))
    for (const [name, value] of Object.entries(values)) {
      const { min, max = Infinity } = styleLimits[name];
      assert(value >= min && value <= max, name);
    }
  assert.deepEqual(
    encodingChoices.map((choice) => choice.value),
    desktop.StreamEncoding.enum,
  );
});

test('Home holds the light card and the setup; the rest is in Settings', async () => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          calibrations: [
            { id: 'movie', name: 'Movie night', correction: noCorrection },
          ],
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    assert.equal($$('form').length, 0, 'no forms on Home');
    // One headline, the lights' state; off, it says what Power will do.
    assert(!$('.display'));
    assert.equal($('.light-calm').textContent, 'Power turns on Screen sync.');
    assert.equal($('.header-link').textContent, 'Settings');
    assert(!$('.diagnostics'));
    assert(!$('.led-map-play'), 'no preview without a preview client');
    assert.deepEqual(
      $$('.light-card input').map((control) => control.name),
      ['processing.brightness_percent'],
    );
    // The setup card: the path from the picture through Prysm to the LEDs.
    assert.deepEqual(
      $$('.path-name').map((item) => item.textContent),
      ['Capture card or camera', 'Prysm', 'WLED'],
    );
    assert.equal(
      $('.led-map figcaption').textContent,
      'Seen from the front: 10 LEDs along the top, 0 on the right, 0 along the bottom and 4 on the left, 14 in all.',
    );
    assert.equal(
      $$('.setup-card a').length,
      0,
      'Open WLED lives in the light card',
    );
    assert.equal($$('.setup-card button').length, 1);
    await edit(field('processing.brightness_percent'), 55);
    await settle();
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 55);

    await click($('.row-link', row('Change setup')));
    assert.equal(window.location.hash, '#/settings');
    await click(button('Home'));
    await openSettings();
    assert.equal(window.location.hash, '#/settings');
    assert.equal(document.activeElement.textContent, 'Settings');
    assert.equal($$('form').length, 0, 'nothing waits for a Save here');
    // One column of rows in three groups; Open WLED stays on Home.
    assert.deepEqual(
      $$('.group h2').map((title) => title.textContent),
      ['Setup', 'Picture', 'This device'],
    );
    assert.deepEqual(
      $$('.row-label, .row-switch label').map((label) => label.textContent),
      [
        'Video source',
        'Lights',
        'Style',
        'Calibrations',
        'Detail',
        'Remove black bars',
        'Appearance',
        'About this Prysm',
        'Start over with setup',
      ],
    );
    assert.equal(
      valueOf('Video source'),
      'Capture card or camera · 640\u00a0×\u00a0360 · 30 fps',
    );
    assert.equal(valueOf('Lights'), 'WLED · 14 LEDs');
    assert.equal(valueOf('Style'), 'Natural');
    assert.equal(
      valueOf('Detail'),
      'How closely the lights follow small details',
    );
    assert(!$('.settings a'), 'Open WLED stays on Home');
    await openEditor('Video source');
    assert.equal(window.location.hash, '#/settings/video');
    assert.equal(document.activeElement.textContent, 'Video source');
    assert(form('Video source'));
    await leaveEditor();
    await openEditor('Lights');
    assert.equal(window.location.hash, '#/settings/lights');
    assert(form('Your lights'));
    await leaveEditor();
    assert.equal(window.location.hash, '#/settings');
    await click(button('Home'));
    assert(atHome(), window.location.hash);
    assert($('.light-card'));
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: {
          state: 'stopped',
          failure: { kind: 'adapter', section: 'output', message: 'no answer' },
        },
      }),
    );
    await click($('.row-link', row('Change setup')));
    assert.equal(window.location.hash, '#/settings/lights');
  } finally {
    await portal.unmount();
  }
});

test('the setup card names devices and sets their facts neatly, never a raw id', async () => {
  const id = '6C707041-05AC-0011-0008-000000000001';
  const discovery = fakeDiscoveryClient({
    'capture/device': [
      { label: 'Elgato Cam Link 4K', fields: { device: id } },
      { label: 'FaceTime HD Camera', fields: { device: 'other' } },
    ],
    'output/wled': [
      { label: 'WLED-Gledopto', fields: { address: '192.0.2.1' } },
    ],
  });
  const portal = await renderPortal(
    HeadlessWebPortal,
    {
      capture: { kind: 'device', device: id, width: 640, height: 360, fps: 30 },
      output: { ...output, leds: { top: 96, right: 54, bottom: 96, left: 54 } },
    },
    undefined,
    { discovery: discovery.client },
  );
  try {
    await act(async () => sleep(0));
    const stops = () =>
      $$('.path-stop').map((stop) => [
        $('.path-name', stop).textContent,
        $('.path-fact', stop)?.textContent,
      ]);
    // Names, not ids; one quiet fact each; the LED count said once.
    assert.deepEqual(stops(), [
      ['Elgato Cam Link 4K', '640 × 360'],
      // Nothing negative: without a calibration the line stays empty.
      ['Prysm', '\u00a0'],
      // A non-breaking hyphen: never split at it.
      ['WLED\u2011Gledopto', '300 LEDs'],
    ]);
    assert(!$('.setup-card').textContent.includes(id), 'no raw device id');
    assert(!$('.led-map-total'), 'the count is not repeated');
    const links = () =>
      $$('.path-link').map((link) => [
        link.dataset.state,
        // The numbers below the hairline, without their description.
        $('.path-numbers', link).firstChild?.textContent || undefined,
      ]);
    // Above each hairline: what it carries, there in every state. A local
    // capture device names nothing; WLED's colours travel by DDP.
    const protocols = () =>
      $$('.path-protocol').map(
        (label) => label.firstChild?.textContent || undefined,
      );
    assert.deepEqual(protocols(), ['Local', 'DDP']);
    // Said in full in the tooltip and for screen readers.
    assert.deepEqual(
      $$('.path-protocol').map((label) => label.title),
      ['Video from a device on this computer', 'DDP over UDP'],
    );
    assert.equal($('.setup-card').dataset.path, 'off');
    assert.deepEqual(links(), [
      ['still', undefined],
      ['still', undefined],
    ]);

    // Frames flowing: the links light up; the first carries the measured rate.
    await act(async () =>
      portal.fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
        input: {
          fps: 29.9,
          receiving: true,
          sent_fps: 29.97,
          latency: { output_ms: 11.6 },
        },
      }),
    );
    assert.equal($('.setup-card').dataset.path, 'flowing');
    // The rate once, Prysm's (the tenth only when it matters); each link only
    // its own delay. The video line stays the picture.
    assert.equal(stops()[0][1], '640\u00a0×\u00a0360');
    assert.equal($('.path-figure').textContent.split(':')[0], '29.9 fps');
    assert.match(
      $('.path-figure').title,
      /^frames per second Prysm processes and sends/,
    );
    assert.deepEqual(links(), [
      ['flowing', undefined],
      ['flowing', '+12 ms'],
    ]);
    // Unstamped source: no total on the Lights stop, only the link's part;
    // the line keeps the LED count, which is the tooltip as well.
    assert(!$$('.path-stop')[2].querySelector('.path-figure'));
    assert.equal(
      $$('.path-stop')[2].querySelector('.path-fact').textContent,
      $$('.path-stop')[2].querySelector('.path-text').title,
    );
    assert.match(
      $$('.path-stop')[2].querySelector('.path-text').title,
      /LEDs$/,
    );
    assert.deepEqual(protocols(), ['Local', 'DDP'], 'unchanged while flowing');
    // Each delay on its own link, said for what it is; never "screen to lights".
    const [into, sent] = $$('.path-numbers');
    assert.equal(into.title, '', 'no capture time: no delay on the video link');
    assert.match(
      sent.title,
      /^adds the time inside Prysm from a frame received/,
    );
    assert(!/screen/i.test(sent.title));
    assert.match(
      sent.textContent,
      /adds the time inside Prysm/,
      'for screen readers',
    );
    // A source that stamps capture: its part on the video link.
    await act(async () =>
      portal.fixture.publish({
        input: {
          fps: 29.97,
          receiving: true,
          sent_fps: 29.97,
          latency: { capture_ms: 88.4, output_ms: 2.4 },
        },
      }),
    );
    // The lag adds up: each link its part, the Lights stop the sum.
    assert.deepEqual(links(), [
      ['flowing', '+88 ms'],
      ['flowing', '+2 ms'],
    ]);
    const lightsStop = $$('.path-stop')[2];
    const total = lightsStop.querySelector('.path-figure');
    assert.equal(total.textContent.split(':')[0], '91 ms behind');
    assert.equal(
      total.title,
      'The lights show each frame about 91 ms after it was captured. The lights’ own delay isn’t included.',
    );
    assert.match(
      total.textContent,
      /about 91 ms after it was captured/,
      'for screen readers',
    );
    // The total takes the LED count's place on the line, alone; the count
    // stays in the stop's tooltip and its screen-reader text.
    const lights = lightsStop.querySelector('.path-fact');
    assert.deepEqual(
      [...lights.children].map((part) => part.className),
      ['path-figure'],
    );
    const leds = lightsStop.querySelector('.path-text').title;
    assert.match(leds, /^\d+ LEDs$/);
    assert(
      lightsStop.querySelector('.path-text').textContent.includes(` ${leds},`),
      'for screen readers',
    );
    assert.equal(
      $('.path-figure').textContent.split(':')[0],
      '30 fps',
      '30.0 is 30',
    );
    assert.match(
      $$('.path-numbers')[0].title,
      /^adds the time from the camera capturing a frame to Prysm receiving it/,
    );
    // Under a millisecond is said, not rounded away, and without the "+":
    // "+<1 ms" reads as two operators.
    await act(async () =>
      portal.fixture.publish({
        input: {
          fps: 29.97,
          receiving: true,
          sent_fps: 29.97,
          latency: { output_ms: 0.4 },
        },
      }),
    );
    assert.deepEqual(links(), [
      ['flowing', undefined],
      ['flowing', '<1 ms'],
    ]);
    // No frame flushed in the window: the rate only, no number made up.
    await act(async () =>
      portal.fixture.publish({
        input: { fps: 29.97, receiving: true, sent_fps: 0 },
      }),
    );
    assert.deepEqual(links()[1], ['flowing', undefined], 'no delay made up');
    await act(async () =>
      portal.fixture.publish({
        input: {
          fps: 29.97,
          receiving: true,
          sent_fps: 29.97,
          latency: { output_ms: 11.6 },
        },
      }),
    );
    await act(async () =>
      portal.fixture.publish({ input: { fps: 0, receiving: false } }),
    );
    assert.deepEqual(links()[0], ['still', undefined]);
    assert.equal(stops()[0][1], 'Waiting for picture');

    // The lights' own effects: Prysm is out of the path.
    await act(async () =>
      portal.fixture.publish({
        target: 'effects',
        session: { state: 'effects' },
        input: undefined,
      }),
    );
    assert.deepEqual(links(), [
      ['still', undefined],
      ['still', undefined],
    ]);
    assert.equal($('.setup-card').dataset.path, 'effects');

    // A failure naming the capture marks the video stop and its link.
    await act(async () =>
      portal.fixture.publish({
        target: 'screen_sync',
        session: {
          state: 'effects',
          failure: { kind: 'invalid', field: 'capture.width', message: 'x' },
        },
      }),
    );
    assert.equal($('.setup-card').dataset.path, 'failed');
    assert.deepEqual(
      $$('.path-stop').map((stop) => stop.dataset.failed),
      ['true', 'false', 'false'],
    );
    assert.deepEqual(links(), [
      ['failed', undefined],
      ['still', undefined],
    ]);
    // An adapter failure says its side: the capture, as on the owner's Mac.
    await act(async () =>
      portal.fixture.publish({
        session: {
          state: 'effects',
          failure: {
            kind: 'adapter',
            section: 'capture',
            message:
              'video device capture failed: capture device does not support 640x400 at 30 Hz',
          },
        },
      }),
    );
    assert.deepEqual(
      $$('.path-stop').map((stop) => stop.dataset.failed),
      ['true', 'false', 'false'],
    );
    assert.equal(
      info(),
      'The video source isn’t working. WLED effects are still on.',
    );
    // The lights' side.
    await act(async () =>
      portal.fixture.publish({
        session: {
          state: 'effects',
          failure: { kind: 'adapter', section: 'output', message: 'x' },
        },
      }),
    );
    assert.deepEqual(
      $$('.path-stop').map((stop) => stop.dataset.failed),
      ['false', 'false', 'true'],
    );
    assert.deepEqual(links(), [
      ['still', undefined],
      ['failed', undefined],
    ]);
    assert.equal(info(), 'The lights aren’t responding.');
    // One without a side: the path is broken at Prysm.
    await act(async () =>
      portal.fixture.publish({
        session: {
          state: 'effects',
          failure: { kind: 'adapter', message: 'x' },
        },
      }),
    );
    assert.deepEqual(
      $$('.path-stop').map((stop) => stop.dataset.failed),
      ['false', 'true', 'false'],
    );
    assert.deepEqual(discovery.calls.sort(), ['capture/device', 'output/wled']);
  } finally {
    await portal.unmount();
  }
});

test('with the lights off, saved lights that do not validate are said on the path, in Settings and on their page', async () => {
  // Off read as off: the setup path, the Lights page and its row said
  // nothing, and Change setup opened Settings.
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await act(async () =>
      fixture.publish({
        target: 'off',
        session: {
          state: 'stopped',
          failure: {
            kind: 'invalid',
            field: 'output.address',
            message: 'a unicast device address is required',
          },
        },
      }),
    );
    assert.equal(word(), 'Off');
    assert.equal(info(), 'Prysm can’t use these settings.');
    assert(!button('Try again'), 'trying could only switch off again');
    assert.equal($('.setup-card').dataset.path, 'failed');
    assert.deepEqual(
      $$('.path-stop').map((stop) => stop.dataset.failed),
      ['false', 'false', 'true'],
    );
    await click($('.row-link', row('Change setup')));
    assert.equal(window.location.hash, '#/settings/lights');
    assert.equal(
      now(),
      'Can’t use these settings — WLED at 192.0.2.1 · 14 LEDs',
    );
    assert.equal($('.section-status').dataset.tone, 'problem');
    assert(text().includes('a unicast device address is required'));
    await leaveEditor();
    assert.equal(problem('Lights'), 'Prysm can’t use these settings.');
    assert.equal(problem('Video source'), '');
    await openEditor('Video source');
    assert.equal(
      now(),
      'Not in use — Screen sync is off, so this source isn’t in use.',
    );
  } finally {
    await portal.unmount();
  }
});

test('a remote capture names its encoding above the first link', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'remote', encoding: 'raw' },
    output,
  });
  try {
    assert.deepEqual(
      $$('.path-protocol').map((label) => label.firstChild.textContent),
      ['Raw', 'DDP'],
    );
    assert.equal($('.path-protocol').title, 'Raw video over TCP');
    // The fact lines stay the picture and the LEDs.
    assert.equal($$('.path-fact')[0].textContent, '320 × 180');
    assert.equal($$('.path-fact')[2].textContent, '14 LEDs');
  } finally {
    await portal.unmount();
  }
});

test('the step that keeps Prysm behind shows in orange; a failure stays red', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const flowing = (behind) =>
    act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
        input: {
          fps: 24.3,
          receiving: true,
          sent_fps: 24.3,
          latency: { output_ms: 38.2 },
          ...(behind ? { behind } : {}),
        },
      }),
    );
  const stopsBehind = () => $$('.path-stop').map((stop) => stop.dataset.behind);
  const linksBehind = () => $$('.path-link').map((link) => link.dataset.behind);
  try {
    // Keeping up: nothing orange.
    await flowing(undefined);
    assert.deepEqual(stopsBehind(), ['false', 'false', 'false']);
    assert.deepEqual(linksBehind(), ['false', 'false']);
    assert.match($('.path-figure').title, /^frames per second Prysm/);

    // Processing: the Prysm stop and its figure, said in plain words.
    await flowing('processing');
    assert.deepEqual(stopsBehind(), ['false', 'true', 'false']);
    assert.deepEqual(linksBehind(), ['false', 'false']);
    const figure = $('.path-figure');
    assert.match(figure.title, /^Prysm can’t keep up: processing each frame/);
    assert.match(
      figure.textContent,
      /Prysm can’t keep up/,
      'for screen readers',
    );

    // Output: the lights link, with its delay.
    await flowing('output');
    assert.deepEqual(stopsBehind(), ['false', 'false', 'false']);
    assert.deepEqual(linksBehind(), ['false', 'true']);
    const sent = $$('.path-numbers')[1];
    assert.match(sent.title, /^Prysm can’t keep up: sending to the lights/);
    assert.match(sent.textContent, /sending to the lights/);
    assert.match($('.path-figure').title, /^frames per second Prysm/);

    // A failure wins: red, never orange, whatever input still says.
    await act(async () =>
      fixture.publish({
        session: {
          state: 'stopped',
          failure: { kind: 'adapter', section: 'output', message: 'gone' },
        },
      }),
    );
    assert.equal($('.setup-card').dataset.path, 'failed');
    assert.deepEqual(stopsBehind(), ['false', 'false', 'false']);
    assert.deepEqual(linksBehind(), ['false', 'false']);
    assert.equal($$('.path-stop')[2].dataset.failed, 'true');
  } finally {
    await portal.unmount();
  }
});

test('discovered names are kept in this browser: shown at once next time, not for another device', async () => {
  const memory = {};
  // A storage the portal reads before discovery answers.
  const withStorage = (snapshot) => {
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: (key) => memory[key] ?? null,
        setItem: (key, value) => {
          memory[key] = String(value);
        },
        removeItem: (key) => {
          delete memory[key];
        },
      },
    });
    return snapshot;
  };
  const lights = { ...output, address: '192.0.2.7' };
  const name = () => $$('.path-name')[2].textContent;
  // First visit: the kind's name until discovery finds the device.
  const found = fakeDiscoveryClient({
    'output/wled': [
      { label: 'WLED-Gledopto', fields: { address: '192.0.2.7' } },
    ],
  });
  let portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output: lights },
    withStorage,
    { discovery: found.client },
  );
  try {
    await act(async () => sleep(0));
    assert.equal(name(), 'WLED‑Gledopto');
    assert.deepEqual(JSON.parse(memory[deviceNamesKey])['output:wled'], [
      { fields: { address: '192.0.2.7' }, name: 'WLED-Gledopto' },
    ]);
  } finally {
    await portal.unmount();
  }
  // Next visit: discovery is slow; the kept name shows at once.
  const slow = { candidates: () => new Promise(() => {}) };
  portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output: lights },
    withStorage,
    { discovery: slow },
  );
  try {
    assert.equal(name(), 'WLED‑Gledopto', 'before discovery answers');
  } finally {
    await portal.unmount();
  }
  // Another device saved: not its name; the old one is kept for its return.
  portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output: { ...output, address: '192.0.2.99' } },
    withStorage,
    { discovery: slow },
  );
  try {
    assert.equal(name(), 'WLED');
    assert.deepEqual(
      JSON.parse(memory[deviceNamesKey])['output:wled'].map(({ name }) => name),
      ['WLED-Gledopto'],
    );
  } finally {
    await portal.unmount();
  }
});

test('calibrations swap with one tap on Home, applied at once', async () => {
  const calibrations = [
    { id: 'tv', name: 'TV', correction: noCorrection },
    { id: 'ps5', name: 'PS5', correction: noCorrection },
  ];
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          calibrations,
          active_calibration: 'tv',
        },
      },
    }),
  );
  const { fixture } = portal;
  const pressed = () =>
    chips()
      .filter((item) => item.getAttribute('aria-checked') === 'true')
      .map((item) => item.textContent);
  try {
    assert.deepEqual(
      chips().map((item) => [item.getAttribute('role'), item.textContent]),
      [
        ['radio', 'TV'],
        ['radio', 'PS5'],
        ['radio', 'No correction'],
      ],
    );
    assert.deepEqual(pressed(), ['TV']);
    assert(button('Calibrate colours'), 'another can always be made');

    await press(chip('ps5'));
    assert.deepEqual(pressed(), ['PS5']);
    assert.equal(saves(fixture), 1, 'no wait');
    assert.equal(
      fixture.snapshot().settings.processing.active_calibration,
      'ps5',
    );
    assert.deepEqual(
      fixture.snapshot().settings.processing.calibrations,
      calibrations,
    );

    await press(chip('ps5'));
    assert.equal(saves(fixture), 1, 'the active one again saves nothing');
    await press(chip('tv'));
    assert.equal(
      fixture.snapshot().settings.processing.active_calibration,
      'tv',
    );
    await press(chip(''));
    assert.deepEqual(pressed(), ['No correction']);
    assert.equal(
      'active_calibration' in fixture.snapshot().settings.processing,
      false,
    );
    assert.equal(saves(fixture), 3);

    // Calibrations correct the screen's colours: quiet for the lights' effects.
    assert.equal($('.picture-controls').hasAttribute('inert'), false);
    await press(mode('effects'));
    assert.equal($('.picture-controls').hasAttribute('inert'), true);
  } finally {
    await portal.unmount();
  }
  // Without calibrations only No correction; Calibrate colours, beside the
  // header, makes one.
  const empty = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    assert.deepEqual(
      chips().map((item) => item.textContent),
      ['No correction'],
    );
    assert.equal(segmentsOf('Calibration').getAttribute('role'), 'radiogroup');
    await press(button('Calibrate colours'));
    assert.equal(window.location.hash, '#/calibrate');
    await press(button('Leave'));
    assert(atHome(), window.location.hash);
  } finally {
    await empty.unmount();
  }
});

test('the style swaps with one tap on Home, applied at once; Adjust opens the Style page', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture, dom } = portal;
  const segment = (style) => $(`[data-value="${style}"]`, segmentsOf('Style'));
  const segments = () => $$('.segment', segmentsOf('Style'));
  const checked = () =>
    segments()
      .filter((item) => item.getAttribute('aria-checked') === 'true')
      .map((item) => item.textContent);
  const saved = () => fixture.snapshot().settings.processing;
  const key = (element, name) =>
    act(async () => {
      element.dispatchEvent(
        new dom.window.KeyboardEvent('keydown', { key: name, bubbles: true }),
      );
      await sleep(0);
    });
  try {
    // A radio group named Style, each segment described by its style's line;
    // Custom only once there is one.
    const group = segmentsOf('Style');
    assert.equal(group.getAttribute('role'), 'radiogroup');
    assert.deepEqual(
      segments().map((item) => [
        item.getAttribute('role'),
        item.textContent,
        hint(item),
        item.tabIndex,
      ]),
      [
        ['radio', 'Natural', 'Calm in quiet scenes, follows cuts at once.', 0],
        ['radio', 'Calm', 'A slow, soft glow. Nothing sudden.', -1],
        ['radio', 'Vivid', 'Stronger colours, follows the action.', -1],
        ['radio', 'Game', 'Reacts at once, with the least delay.', -1],
      ],
    );
    assert.deepEqual(checked(), ['Natural']);
    // Under the mode, quiet with the calibrations for the lights' effects.
    assert($('.picture-controls').contains(group));

    // One tap: shown chosen at once, saved without a wait.
    await press(segment('game'));
    assert.deepEqual(checked(), ['Game']);
    assert.equal(saves(fixture), 1, 'no wait');
    assert.deepEqual(saved().style, { kind: 'game' });
    assert.equal(segment('game').tabIndex, 0);
    await press(segment('game'));
    assert.equal(saves(fixture), 1, 'the chosen one again saves nothing');

    // Arrow keys move the choice, wrapping, and the focus follows it.
    segment('game').focus();
    await key(segment('game'), 'ArrowRight');
    assert.deepEqual(checked(), ['Natural']);
    assertFocused(segment('natural'));
    await key(segment('natural'), 'ArrowLeft');
    assert.deepEqual(checked(), ['Game']);
    assertFocused(segment('game'));
    await settle();
    assert.deepEqual(saved().style, { kind: 'game' });

    // Kept values offer Custom, which returns to them.
    const kept = { ...styleValues.calm, dark_floor_percent: 15 };
    await elsewhere(fixture, {
      processing: { ...saved(), custom_style: kept },
    });
    assert.deepEqual(
      segments().map((item) => item.textContent),
      ['Natural', 'Calm', 'Vivid', 'Game', 'Custom'],
    );
    await press(segment('custom'));
    await settle();
    assert.deepEqual(saved().style, { kind: 'custom' });
    assert.deepEqual(saved().custom_style, kept);

    // A refused change is said as a refused calibration is, under the card.
    fixture.failNext({
      kind: 'adapter',
      message: 'settings file is read-only',
    });
    await press(segment('calm'));
    await settle();
    assert.equal(
      $('.quick-picture .problem-line').textContent,
      'Not saved. Try again',
    );
    assert.deepEqual(saved().style, { kind: 'custom' });

    await press(button('Adjust style'));
    assert.equal(window.location.hash, '#/settings/style');
  } finally {
    await portal.unmount();
  }
});

test('the theme choice sets data-theme, is kept in this browser and works without storage', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    await openSettings();
    assert.equal(valueOf('Appearance'), 'System');
    await openPage('Appearance');
    assert.equal(window.location.hash, '#/settings/appearance');
    const choice = (label) => $('input', row(label));
    assert.equal(choice('System').checked, true);
    // Storage refused (a private window, blocked site data): the choice
    // still applies.
    Object.defineProperty(window, 'localStorage', {
      get() {
        throw new window.DOMException('Blocked', 'SecurityError');
      },
    });
    await click(choice('Dark'));
    assert.equal(document.documentElement.getAttribute('data-theme'), 'dark');
    assert.equal(choice('Dark').checked, true);
    assert.equal(storedTheme(), 'system', 'nothing kept without storage');
    await click(button('Settings'));
    assert.equal(valueOf('Appearance'), 'Dark');
    await openPage('Appearance');
    await click(choice('System'));
    assert.equal(document.documentElement.hasAttribute('data-theme'), false);
  } finally {
    await portal.unmount();
  }
  // With storage the choice is kept and applied on the next visit.
  const dom = new JSDOM('', { url: 'http://prysm.test/' });
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
  });
  try {
    saveTheme('light');
    assert.equal(dom.window.localStorage.getItem(themeStorageKey), 'light');
    assert.equal(storedTheme(), 'light');
    applyTheme(storedTheme());
    assert.equal(document.documentElement.getAttribute('data-theme'), 'light');
    saveTheme('system');
    assert.equal(dom.window.localStorage.getItem(themeStorageKey), null);
    assert.equal(storedTheme(), 'system');
  } finally {
    dom.window.close();
  }
});

/** Let answered requests and renders run without moving the mocked clock. */
const flush = () =>
  act(async () => {
    for (let i = 0; i < 5; i++)
      await new Promise((resolve) => setImmediate(resolve));
  });

test('slider trials go at most every interval, one at a time, the latest winning', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: 0 });
  const sent = [];
  const answers = [];
  const send = (value) => {
    sent.push([value, Date.now()]);
    return new Promise((resolve, reject) => answers.push({ resolve, reject }));
  };
  const trials = throttled(trialIntervalMs);
  assert(trialIntervalMs >= 80 && trialIntervalMs <= 120);

  // The first goes at once; nothing more while it is unanswered, however long.
  trials.push(1, send);
  assert.deepEqual(sent, [[1, 0]]);
  for (let at = 10; at <= 300; at += 10) {
    t.mock.timers.tick(10);
    trials.push(at, send);
  }
  assert.equal(sent.length, 1);
  // Answered (even with a failure): only the latest goes, the interval long past.
  answers[0].reject(new Error('lost'));
  await flush();
  assert.deepEqual(sent[1], [300, 300]);

  // Answered at once: one per interval, the trailing value last.
  answers[1].resolve();
  await flush();
  for (let at = 310; at <= 1300; at += 10) {
    t.mock.timers.tick(10);
    trials.push(at, send);
    answers.at(-1).resolve();
    await flush();
  }
  t.mock.timers.tick(trialIntervalMs);
  answers.at(-1).resolve();
  await flush();
  const times = sent.map(([, at]) => at);
  times
    .slice(1)
    .forEach((at, i) =>
      assert(at - times[i] >= trialIntervalMs, `${at} after ${times[i]}`),
    );
  assert(sent.length <= 2 + 1000 / trialIntervalMs + 1, `${sent.length} sent`);
  assert.equal(sent.at(-1)[0], 1300, 'the trailing value is sent');

  // `settle` drops what waits and resolves once nothing is in flight.
  t.mock.timers.tick(trialIntervalMs);
  trials.push(1301, send);
  trials.push(1302, send);
  let settled = false;
  const settling = trials.settle().then(() => (settled = true));
  await flush();
  assert.equal(sent.at(-1)[0], 1301);
  assert(!settled, 'waits for the one in flight');
  answers.at(-1).resolve();
  await settling;
  t.mock.timers.tick(1000);
  await flush();
  assert.equal(sent.at(-1)[0], 1301, 'the dropped value is never sent');
});

test('quick separate taps on Home brightness always save the last value', async (t) => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture, dom } = portal;
  const view = dom.window;
  const slider = () => field('processing.brightness_percent');
  const fire = (type, init = {}) =>
    act(async () =>
      slider().dispatchEvent(new view.Event(type, { bubbles: true, ...init })),
    );
  const saved = () => fixture.snapshot().settings.processing.brightness_percent;
  try {
    await press(mode('screen_sync'));
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: 0 });
    for (const answer of ['at once', 'slowly']) {
      const start = saved();
      // Slow answers: each tap's save is still on its way at the next tap.
      const release = answer === 'slowly' ? fixture.holdSaves() : () => {};
      for (let step = 1; step <= 5; step++) {
        await fire('keydown', { key: 'ArrowLeft' });
        await edit(slider(), start - step);
        await fire('keyup', { key: 'ArrowLeft' });
        await flush();
        t.mock.timers.tick(40);
        await flush();
      }
      release();
      for (let wait = 0; wait < 6; wait++) {
        await flush();
        t.mock.timers.tick(processingDelayMs);
      }
      await flush();
      assert.equal(slider().value, String(start - 5), answer);
      assert.equal(saved(), start - 5, `${answer}: the last value is saved`);
    }
  } finally {
    t.mock.timers.reset();
    await portal.unmount();
  }
});

test('dragging Home brightness lights as it moves and saves once, where it is let go', async (t) => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture, dom } = portal;
  const view = dom.window;
  const slider = () => field('processing.brightness_percent');
  const fire = (type, init = {}) =>
    act(async () =>
      slider().dispatchEvent(new view.Event(type, { bubbles: true, ...init })),
    );
  const trials = () =>
    fixture.calls
      .filter((call) => call.startsWith('trial:'))
      .map((call) => Number(call.slice(6)));
  try {
    await press(mode('screen_sync'));
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: 0 });
    const revision = fixture.snapshot().revision;
    const card = () => ({
      word: word(),
      note: note(),
      info: info(),
      busy: power().getAttribute('aria-busy'),
      mood: $('.light-card').dataset.mood,
    });
    const calm = card();

    // Pointer: the first value goes at once, then one at a time.
    const release = fixture.holdTrials();
    await fire('pointerdown');
    await edit(slider(), 60);
    assert.deepEqual(trials(), [60]);
    for (let value = 61; value <= 70; value++) {
      t.mock.timers.tick(20);
      await edit(slider(), value);
      assert.equal(slider().value, String(value), 'the thumb never jumps back');
    }
    assert.deepEqual(trials(), [60], 'one in flight');
    release();
    await flush();
    assert.deepEqual(trials(), [60, 70], 'then the latest value');
    assert.equal(fixture.trials.mostInFlight, 1);

    // Answered at once: at most one per interval. A failed step shows nothing.
    fixture.failTrials({ kind: 'adapter', message: 'lights unreachable' });
    for (let value = 71; value <= 90; value++) {
      t.mock.timers.tick(25);
      await edit(slider(), value);
      await flush();
    }
    t.mock.timers.tick(trialIntervalMs);
    await flush();
    assert(
      trials().length <= 2 + (20 * 25) / trialIntervalMs + 1,
      `${trials()}`,
    );
    assert.equal(trials().at(-1), 90, 'the trailing value is tried');
    fixture.failTrials(null);
    // Held still for long: still nothing saved, nothing in the card moved.
    t.mock.timers.tick(processingDelayMs * 5);
    await flush();
    assert.equal(saves(fixture), 0);
    assert.equal(fixture.snapshot().revision, revision);
    assert.deepEqual(card(), calm);
    assert(!text().includes('Not saved'));
    assert.equal(slider().value, '90');

    // Let go: the final value is saved, once, after the trials.
    const before = trials().length;
    await fire('pointerup');
    await flush();
    assert.equal(saves(fixture), 1);
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 90);
    assert.equal(fixture.calls.at(-1), 'replaceSettings');
    assert.equal(trials().length, before, 'no trial after the save');
    assert.deepEqual(card(), calm);
    assert.equal(slider().value, '90');

    // Keys the same: tried as they repeat, saved when the key comes up.
    await fire('keydown', { key: 'ArrowLeft' });
    for (let value = 89; value >= 80; value--) {
      t.mock.timers.tick(30);
      await edit(slider(), value);
      await flush();
    }
    t.mock.timers.tick(trialIntervalMs);
    await flush();
    assert.equal(trials().at(-1), 80);
    assert(trials().length - before <= 1 + (10 * 30) / trialIntervalMs + 1);
    assert.equal(saves(fixture), 1);
    await fire('keyup', { key: 'ArrowLeft' });
    await flush();
    assert.equal(saves(fixture), 2);
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 80);

    // Only the final value's failure is reported, as before.
    fixture.failNext({
      kind: 'adapter',
      message: 'settings file is read-only',
    });
    await fire('pointerdown');
    await edit(slider(), 40);
    assert(!text().includes('Not saved'));
    await fire('pointerup');
    await flush();
    assert.equal(
      $('.quick-picture .problem-line').textContent,
      'Not saved. Try again',
    );
    assert.equal(slider().value, '40', 'the thumb stays where it was let go');
  } finally {
    t.mock.timers.reset();
    await portal.unmount();
  }
});

test('Adjust this style opens by hand on a built-in style; moving a slider there makes it Custom from that style', async () => {
  const kept = { ...styleValues.calm, dark_floor_percent: 15 };
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          style: { kind: 'vivid' },
          custom_style: kept,
        },
      },
    }),
  );
  const { fixture } = portal;
  const adjust = () => $('button', row('Adjust this style'));
  const knob = (name) => field(`processing.custom_style.${name}`);
  const ticked = () =>
    $$('.row-choice')
      .filter((choice) => $('input', choice).checked)
      .map((choice) => $('.row-label', choice).textContent);
  try {
    await openPage('Style');
    // Closed on a built-in style; a real button, opened and closed by hand.
    assert.equal(adjust().getAttribute('aria-expanded'), 'false');
    const body = () =>
      document.getElementById(adjust().getAttribute('aria-controls'));
    assert(body(), 'the button names what it opens');
    assert.equal($$('input[type="range"]', body()).length, 0);
    await click(adjust());
    assert.equal(adjust().getAttribute('aria-expanded'), 'true');
    assert.equal($$('input[type="range"]', body()).length, 7);
    await click(adjust());
    assert.equal(adjust().getAttribute('aria-expanded'), 'false');
    assert.equal($$('input[type="range"]').length, 0);
    await click(adjust());

    // Open, it shows the ticked style's own values; nothing is saved.
    for (const [name, value] of Object.entries(styleValues.vivid))
      assert.equal(Number(knob(name).value), value, name);
    assert.equal(saves(fixture), 0);

    // Moving one makes it Custom: Vivid's values with the change, one save
    // that replaces the kept values. The tick moves; the sliders stay open.
    await edit(knob('spatial_reach_percent'), 42);
    assert.deepEqual(ticked(), ['Custom']);
    assert.equal(adjust().getAttribute('aria-expanded'), 'true');
    await settle();
    assert.equal(saves(fixture), 1);
    const changed = { ...styleValues.vivid, spatial_reach_percent: 42 };
    assert.deepEqual(
      [
        fixture.snapshot().settings.processing.style,
        fixture.snapshot().settings.processing.custom_style,
      ],
      [{ kind: 'custom' }, changed],
    );
    assert.equal(knob('spatial_reach_percent').value, '42');

    // Arriving with Custom chosen, the sliders are open on the kept values.
    await leaveEditor();
    await openPage('Style');
    assert.equal(adjust().getAttribute('aria-expanded'), 'true');
    assert.equal(knob('spatial_reach_percent').value, '42');

    // A built-in style and back: Custom returns to the values kept now.
    await click(radio('Game'));
    assert.equal(
      Number(knob('spatial_reach_percent').value),
      styleValues.game.spatial_reach_percent,
    );
    await click(radio('Custom'));
    assert.deepEqual(ticked(), ['Custom']);
    assert.deepEqual(
      fixture.snapshot().settings.processing.custom_style,
      changed,
    );
    assert.equal(knob('spatial_reach_percent').value, '42');
  } finally {
    await portal.unmount();
  }
});

for (const [name, label, value] of [
  ['color_focus_percent', 'Colour focus', 60],
  ['cut_response_percent', 'Scene cuts', 70],
  ['brightness_ramp_seconds', 'Brightness ramp', 1.2],
]) {
  test(`dragging ${label} under Custom lights as it moves and saves where it is let go`, async () => {
    const custom_style = { ...styleValues.natural };
    const portal = await renderPortal(
      HeadlessWebPortal,
      { capture, output },
      (snapshot) => ({
        settings: {
          ...snapshot.settings,
          processing: {
            ...snapshot.settings.processing,
            style: { kind: 'custom' },
            custom_style,
          },
        },
      }),
    );
    const { fixture, dom } = portal;
    const slider = () => field(`processing.custom_style.${name}`);
    const fire = (type) =>
      act(async () =>
        slider().dispatchEvent(new dom.window.Event(type, { bubbles: true })),
      );
    const trials = () =>
      fixture.calls.filter((call) => call.startsWith('trial:')).length;
    try {
      await press(mode('screen_sync'));
      await openSettings();
      await openPage('Style');
      await fire('pointerdown');
      await edit(slider(), value);
      await flush();
      assert.equal(trials(), 1, 'tried on the lights at once');
      assert.equal(saves(fixture), 0, 'not saved while held');
      await fire('pointerup');
      await flush();
      assert.equal(saves(fixture), 1);
      assert.deepEqual(fixture.snapshot().settings.processing.custom_style, {
        ...custom_style,
        [name]: value,
      });
    } finally {
      await portal.unmount();
    }
  });
}

test('with the state in the page, the first render is Home, whole and live, before the stream answers', async () => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    {},
    { embedded: true, held: true },
  );
  try {
    assert(!$('.skeleton'), 'no waiting');
    assert($('.portal').classList.contains('wide'));
    assert($('.light-card') && $('.setup-card'), 'both cards');
    assert.equal(power().disabled, false, 'live at once');
    assert(button('Settings'));
    assert.equal($('.connection').textContent, 'Connected');
    // The stream's first state takes over in place.
    const card = $('.light-card');
    await portal.answer();
    assertSame($('.light-card'), card, 'the same card');
    assert.deepEqual(portal.fixture.calls, ['observe']);
  } finally {
    await portal.unmount();
  }
});

test('with the state in the page, a refused stream keeps Home and says the refusal, not a reconnect', async () => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    {},
    { embedded: true, held: true },
  );
  const refusal = 'Prysm refuses requests from this page.';
  try {
    await portal.answer({ kind: 'forbidden', message: refusal });
    await act(async () => sleep(connectionGraceMs + 100));
    assert($('.light-card') && $('.setup-card'), 'Home stays');
    assert.equal($('.notice[role="alert"]').textContent, refusal);
    assert.equal($('.connection').textContent, 'Offline');
    assert(!document.body.textContent.includes('Reconnecting'));
  } finally {
    await portal.unmount();
  }
});

test('without a state in the page, Home waits in its own frame, says so only once the wait lasts, and the cards take the blocks’ places', async () => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    {},
    { held: true },
  );
  const message = () => $('.skeleton-message').textContent;
  try {
    // Home's frame as it will be: wide, the header with Settings, a block
    // for each card, nothing said yet.
    assert($('.portal').classList.contains('wide'));
    assert(button('Settings'));
    assert.equal($$('.home-grid > .skeleton').length, 2);
    assert(!$('.light-card'));
    assert.equal(message(), '');
    assert.equal($('.connection').className, 'connection online');
    await act(async () => sleep(connectionGraceMs + 100));
    assert.equal(message(), 'Connecting to Prysm…');
    assert.equal($('.connection').textContent, 'Connecting…');
    await portal.answer();
    assert(!$('.skeleton'));
    assert($('.portal').classList.contains('wide'));
    assert($('.light-card') && $('.setup-card'), 'both cards');
    assert.equal(power().disabled, false);
  } finally {
    await portal.unmount();
  }
});

test('a page that fails to draw says so and offers a reload', async (t) => {
  // React reports the failure it caught; not this test's news.
  t.mock.method(console, 'error', () => {});
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    // A state no page can draw.
    await act(async () => portal.fixture.publish({ settings: null }));
    assert.equal($('h2').textContent, 'Something went wrong');
    assert(button('Reload'), 'the way on');
    assert($('.detail').textContent, 'what failed, for a report');
  } finally {
    await portal.unmount();
  }
});

test('after Prysm is updated, every page offers a reload once it reconnects', async () => {
  let replaced = false;
  let asked = 0;
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    undefined,
    {
      replaced: async () => {
        asked += 1;
        return replaced;
      },
    },
  );
  const reconnect = async () => {
    await act(async () => portal.fixture.drop());
    await act(async () => sleep(400));
    assert.equal($('.connection').textContent, 'Connected');
  };
  try {
    assert.equal(asked, 0, 'not asked while the connection holds');
    // A restart without an update: asked, nothing shown.
    await reconnect();
    assert.equal(asked, 1);
    assert(!$('.notice.updated'));
    // An update: said on every page, with the way on.
    replaced = true;
    await reconnect();
    assert.equal($('.notice.updated p').textContent, 'Prysm was updated.');
    assert(button('Reload'));
    await openSettings();
    assert($('.notice.updated'), 'on Settings too');
  } finally {
    await portal.unmount();
  }
});

test('the script that sets the theme before the page is drawn reads the same key', async () => {
  const script = await readFile(
    new URL('../packages/web-portal/public/theme.js', import.meta.url),
    'utf8',
  );
  assert(script.includes(`getItem('${themeStorageKey}')`));
});

test('a target or session state newer than the page chooses no mode, and Power switches off', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture, dom } = portal;
  const key = (element, name) =>
    act(async () => {
      element.dispatchEvent(
        new dom.window.KeyboardEvent('keydown', { key: name, bubbles: true }),
      );
      await sleep(0);
    });
  try {
    await act(async () =>
      fixture.publish({ target: 'unknown', session: { state: 'stopped' } }),
    );
    assert(
      $$('.modes [role="radio"]').every(
        (item) => item.getAttribute('aria-checked') === 'false',
      ),
      'no tile is chosen',
    );
    assert.equal($('.modes').dataset.mode, undefined, 'no outline');
    assert.equal(word(), 'Unknown');
    assert.equal(
      info(),
      'Prysm is newer than this page. Reload it to see what the lights do.',
    );
    assert.equal(power().getAttribute('aria-pressed'), 'true');
    assert.equal(mode('screen_sync').tabIndex, 0, 'the first tile takes focus');
    assert.notEqual(
      $('.picture-controls').dataset.quiet,
      'true',
      'no remembered mode quiets them',
    );
    // The arrows step from the focused tile, so either mode is reached.
    await key(mode('screen_sync'), 'ArrowRight');
    assert.deepEqual(
      fixture.calls.filter((call) => call.startsWith('target:')).slice(-1),
      ['target:effects'],
    );
    await act(async () =>
      fixture.publish({ target: 'unknown', session: { state: 'stopped' } }),
    );
    await press(power());
    assert.deepEqual(
      fixture.calls.filter((call) => call.startsWith('target:')).slice(-1),
      ['target:off'],
    );
    // A known target with a newer session: the section pages say so, never
    // "Starting…".
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'unknown' },
      }),
    );
    await openEditor('Lights');
    assert.equal(
      now(),
      'Prysm is newer than this page — Reload it to see what the lights do.',
    );
  } finally {
    await portal.unmount();
  }
});

test('a faulted session says what did not finish on the section pages', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: {
          state: 'faulted',
          message: 'The video source did not close in time.',
        },
      }),
    );
    await openEditor('Video source');
    assert.match(now(), /^No picture/);
    assert(
      document.body.textContent.includes(
        'The video source did not close in time.',
      ),
      'the fault’s words under Details',
    );
  } finally {
    await portal.unmount();
  }
});
