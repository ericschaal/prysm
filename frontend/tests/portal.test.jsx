import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import {
  correctionLimits,
  deviceNamesKey,
  hueLimits,
  processingDelayMs,
  processingLimits,
  profileLimits,
  rebased,
  saveDeveloper,
  savedForMs,
  throttled,
  trialIntervalMs,
  ringDelayMs,
  workingAfterMs,
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
import { deviceDefaults, deviceLimits } from '@prysm/capture-device-ui';
import { fileDefaults, fileLimits } from '@prysm/capture-file-ui';
import {
  includeMenusHelp,
  largeSizeHint,
  tvPictureLimits,
  webosDefaults,
} from '@prysm/capture-webos-ui';
import {
  encodingChoices,
  largeStreamHint,
  remoteDefaults,
} from '@prysm/capture-remote-ui';
import { defaultProcessing } from '../testkit/application';
import { fakeDiscoveryClient } from '../testkit/discovery';
import {
  $,
  $$,
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
  row,
  valueOf,
  renderPortal,
  sleep,
  submit,
  text,
} from './dom';

const capture = { kind: 'device', path: '/dev/video0' };
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
const chip = (id) => $(`[data-profile="${id}"]`);
/** The card's calm line: what Power will do, a pause that lasts, what runs. */
const note = () => $('.light-calm').textContent || null;
const hint = (element) =>
  document.getElementById(element.getAttribute('aria-describedby')).textContent;
/** The editor's one status line. */
const status = () => $('.form-status').textContent;
const saves = (fixture) =>
  fixture.calls.filter((call) => call === 'replaceSettings').length;
const press = (element) =>
  act(async () => {
    element.click();
    await sleep(0);
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
        target: 'processed',
        session: { state: 'running' },
        output_issue: 'overridden',
      }),
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Lights follow your screen',
    );
    assert.match(info(), /^Live override is on/);
    assert.equal($('.light-info').dataset.tone, 'warning');
    assert.equal($('.light-card [role="alert"]'), null, 'not a failure');

    await act(async () => fixture.publish({ output_issue: 'off' }));
    assert.match(info(), /switched off on the device/);

    await act(async () => fixture.publish({ output_issue: undefined }));
    assert.equal(info(), null, 'cleared once the lights follow');
    // It folds away with its last words, out of the tab order.
    assert.equal($('.light-info > div').hasAttribute('inert'), true);
  } finally {
    await portal.unmount();
  }
});

test('the lighting card names a low WLED brightness while screen sync shows', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    const running = {
      target: 'processed',
      session: { state: 'running' },
      input: { fps: 30, receiving: true },
    };
    await act(async () =>
      fixture.publish({ ...running, output_brightness: { percent: 40 } }),
    );
    // In place of the calm line, calm itself: a note, not a warning.
    assert.equal(info(), 'WLED limits brightness to 40%.');
    assert.equal($('.light-info').dataset.tone, 'note');
    assert.equal($('.light-calm').dataset.shown, 'false');

    // High enough to hardly see: nothing to say.
    await act(async () =>
      fixture.publish({ output_brightness: { percent: 80 } }),
    );
    assert.equal(info(), null);
    assert.equal($('.light-calm').dataset.shown, 'true');

    // A device that does not show the colours says that instead.
    await act(async () =>
      fixture.publish({
        output_brightness: { percent: 40 },
        output_issue: 'overridden',
      }),
    );
    assert.match(info(), /^Live override is on/);

    // WLED's own effects use its brightness anyway: no note.
    await act(async () =>
      fixture.publish({
        target: 'device_native',
        session: { state: 'native' },
        input: undefined,
        output_issue: undefined,
      }),
    );
    assert.equal(info(), null);
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
        target: 'processed',
        session: { state: 'running' },
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
      $$('[role="radio"]').map((item) => [
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
    assert.equal($('.modes').dataset.mode, 'processed', 'power turns on sync');
    assert.equal($('.light-card').dataset.lit, 'false');
    assert.equal(info(), null, 'no sentence in the normal states');

    // Power turns screen sync on; the light, and the word, come with the session.
    await press(power());
    assert.deepEqual(targets(), ['target:processed']);
    assert.equal(power().getAttribute('aria-pressed'), 'true');
    assert.equal(mode('processed').getAttribute('aria-checked'), 'true');
    assert.equal(word(), 'On');
    assert.equal($('.light-card').dataset.mood, 'sync');
    assert.equal($('.light-card').dataset.lit, 'true');
    assert.equal(info(), null);
    assert.equal(
      $('.lighting-status').textContent,
      'Lights follow your screen',
    );

    // The tile that is already on restarts nothing.
    await press(mode('processed'));
    assert.equal(targets().length, 1);

    // The other option switches mode while on. Prysm's picture controls do
    // not reach the lights' own effects: they stay in place, quiet and out of
    // reach, and the card's line says so with the way to WLED.
    await press(mode('device_native'));
    assert.deepEqual(targets().slice(-1), ['target:device_native']);
    assert.equal($('.lighting-status').textContent, 'WLED effects are on');
    assert.equal($('.light-card').dataset.mood, 'effects');
    assert.equal(mode('device_native').getAttribute('aria-checked'), 'true');
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
    assert.equal($('.modes').dataset.mode, 'device_native');
    await press(power());
    assert.deepEqual(targets().slice(-1), ['target:device_native']);
    await press(power());

    // A tile chosen while off turns that mode on.
    await press(mode('processed'));
    assert.deepEqual(targets().slice(-2), ['target:off', 'target:processed']);
    assert.equal(word(), 'On');

    await act(async () =>
      fixture.publish({
        target: 'processed',
        session: { state: 'stopped' },
        failure: {
          kind: 'adapter',
          message: 'WLED at 192.0.2.1 did not switch to live mode',
        },
      }),
    );
    assert.equal($('.lighting-status').textContent, 'Screen sync stopped');
    assert($('.lighting-status').classList.contains('visually-hidden'));
    assert.equal($$('.light-card .notice').length, 0, 'one sentence, no box');
    // Plain words from the typed failure; the message under Details.
    assert.equal(info(), 'Screen sync could not start.');
    assert.equal(word(), 'Stopped');
    assert.equal(
      $('.light-card [role="alert"]').textContent,
      'Screen sync could not start.',
    );
    assert.equal(
      $('.light-details p').textContent,
      'WLED at 192.0.2.1 did not switch to live mode',
    );
    assert.equal(power().getAttribute('aria-pressed'), 'true', 'still on');

    // After a failure Try again, or the screen sync tile, retries.
    fixture.failNext({ kind: 'adapter', message: 'fake output cannot open' });
    await press(button('Try again'));
    assert.deepEqual(targets().slice(-1), ['target:processed']);
    assert(text().includes('fake output cannot open'));
    fixture.failNext({ kind: 'adapter', message: 'still cannot open' });
    await press(mode('processed'));
    assert.equal(targets().length, 8);
    assert(text().includes('still cannot open'));

    await act(async () =>
      fixture.publish({
        session: { state: 'faulted', phase: 'closing_source' },
        failure: null,
      }),
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Prysm stopped responding while closing the capture source. Restart Prysm to recover.',
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
      fixture.publish({ target: 'processed', session: { state: 'stopped' } }),
    );
    assert.equal(power().getAttribute('aria-pressed'), 'true');
    assert.equal(word(), 'Off', 'no "On" before the light');
    assert.equal($('.light-card').dataset.mood, 'starting');
    assert.equal(info(), null, 'a quick start says nothing');
    assert.equal($('.light-card').dataset.ignite, undefined, 'not yet lit');
    await act(async () => sleep(slowStartMs + 50));
    assert.equal(info(), null, 'not a problem');
    assert.equal(note(), 'Starting…');

    await act(async () => fixture.publish({ session: { state: 'running' } }));
    assert.equal(word(), 'On');
    assert.equal(info(), null);
    // The ignition plays as the light arrives.
    assert.equal($('.light-card').dataset.ignite, 'a');

    // Switching modes passes through starting without the light going out.
    await act(async () =>
      fixture.publish({
        target: 'device_native',
        session: { state: 'stopped' },
      }),
    );
    assert.equal($('.light-card').dataset.lit, 'true');
    assert.equal(word(), 'On');
    await act(async () => sleep(slowStartMs + 50));
    assert.equal(info(), null, 'still lit: no problem to say');
    // The picture controls go quiet for the lights' own effects, and say so.
    assert.match(note(), /^WLED controls its own effects\./);
    await act(async () => fixture.publish({ session: { state: 'native' } }));
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
      fixture.publish({ target: 'processed', session: { state: 'running' } }),
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
        target: 'device_native',
        session: { state: 'native' },
      }),
    );
    assert.equal(word(), 'On');
    assert.equal($('.light-frame').dataset.palette, 'effects');
    // As the engine does (`set_target` then a failed `start`): first the
    // target changes while the session stays native …
    await act(async () => fixture.publish({ target: 'processed' }));
    assert.equal($('.light-card').dataset.mood, 'starting');
    // … then the failure is recorded; the session is still native.
    await act(async () =>
      fixture.publish({
        failure: {
          kind: 'adapter',
          message:
            'video device capture failed: capture device does not support 640x360 at 30 Hz',
        },
      }),
    );
    assert.equal(fixture.snapshot().session.state, 'native');
    assert.equal(word(), 'Stopped');
    assert.equal($('.light-card').dataset.lit, 'false');
    assert.equal($('.light-card').dataset.mood, 'problem');
    assert.equal(
      $('.light-frame').dataset.palette,
      'error',
      'the warning lamp',
    );
    assert.equal(info(), 'Screen sync could not start.');
    assert.match($('.light-details p').textContent, /does not support 640x360/);
    assert(button('Try again'));
    // A field names the section: said as the video source.
    await act(async () =>
      fixture.publish({
        failure: {
          kind: 'invalid',
          field: 'capture.width',
          message: 'unsupported size',
        },
      }),
    );
    assert.equal(info(), 'The video source is not working.');
    // Off clears it.
    await act(async () =>
      fixture.publish({
        target: 'off',
        session: { state: 'stopped' },
        failure: null,
      }),
    );
    assert.equal(word(), 'Off');
    assert.notEqual($('.light-frame').dataset.palette, 'error');
  } finally {
    await portal.unmount();
  }
});

test('a brightness save while stopped keeps Stopped, the red light and Try again', async () => {
  const capture = { kind: 'device', path: '/dev/video0' };
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
    assert.equal(info(), 'Screen sync could not start.');
    assert(button('Try again'));
  };
  try {
    await act(async () =>
      fixture.publish({
        target: 'device_native',
        session: { state: 'native' },
      }),
    );
    await act(async () => fixture.publish({ target: 'processed' }));
    await act(async () => fixture.publish({ failure }));
    stopped();
    // The save (as the engine now does it): new settings and revision, then
    // the retry's result. The failure is never cleared in between.
    await edit(field('processing.brightness_percent'), 75);
    await settle();
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 75);
    await act(async () => fixture.publish({ failure: { ...failure } }));
    await act(async () => sleep(slowStartMs + 100));
    stopped();
    assert(!text().includes('are on'), 'no session sentence');

    // Starting from effects that takes a while says what is starting.
    await act(async () => fixture.publish({ failure: null }));
    assert.equal($('.light-card').dataset.mood, 'starting');
    await act(async () => sleep(slowStartMs + 100));
    assert.equal(note(), 'Starting…');
    assert(!text().includes('WLED effects are on'));

    // Off with a failure left over: no failure sentence under "Off".
    await act(async () =>
      fixture.publish({
        target: 'off',
        session: { state: 'stopped' },
        failure,
      }),
    );
    assert.equal(word(), 'Off');
    assert.equal(info(), null);
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
    assert.equal(mode('processed').disabled, false);
    assert.equal(mode('device_native').disabled, true);
    assert.equal(
      hint(mode('device_native')),
      'These lights have no effects of their own.',
    );
    assert.equal(
      $('.mode-name', mode('device_native')).textContent,
      'Device effects',
    );
    assert.equal(
      $('.mode-line', mode('device_native')).textContent,
      'Its own patterns',
    );
    assert.equal(power().disabled, false);
    assert.equal($('.device-link'), null, 'no device to open');
  } finally {
    await portal.unmount();
  }
  for (const initial of [{ output }, { capture }, {}]) {
    const unset = await renderPortal(HeadlessWebPortal, initial);
    try {
      await click(button('Leave setup'));
      // A calm card: the prompt and its one action, nothing disabled to puzzle over.
      assert.equal($$('[data-target]').length, 0);
      assert.equal(power(), null);
      assert.equal($('.light-card input'), null);
      assert.equal($('.chip'), null);
      assert.equal(button('Set up Prysm').disabled, false);
      assert.equal($('.setup-card'), null);
      assert.equal(word(), 'Off');
    } finally {
      await unset.unmount();
    }
  }
});

test('a stale picture change keeps both changes: mine where I changed, theirs elsewhere', () => {
  const base = { ...defaultProcessing, brightness_percent: 80 };
  const mine = { ...base, shadow_lift_percent: 30, brightness_percent: 60 };
  const theirs = { ...base, smoothing_seconds: 0.5, brightness_percent: 90 };
  assert.deepEqual(rebased(theirs, base, mine), {
    ...base,
    shadow_lift_percent: 30,
    smoothing_seconds: 0.5,
    // The same control on both: the value the user sees wins.
    brightness_percent: 60,
  });
  // A removed list stays removed; an untouched one follows theirs.
  const profiles = [{ id: 'a', name: 'A' }];
  assert.deepEqual(
    rebased({ ...base, profiles }, { ...base, profiles }, base),
    base,
  );
  assert.deepEqual(rebased({ ...base, profiles }, base, base), {
    ...base,
    profiles,
  });
});

test('Settings has one save status; a refused stale change is saved again on the newer settings', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const status = () => $('.save-status').textContent;
  try {
    await openSettings();
    assert.equal(status(), 'All changes saved');
    await openPage('Fine-tuning');
    assert.equal(window.location.hash, '#/settings/picture');
    assert.equal(status(), 'All changes saved');
    await edit(field('processing.shadow_lift_percent'), 30);
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
    await edit(field('processing.shadow_lift_percent'), 40);
    await settle();
    assert.equal(saves(fixture), 2);
    assert.equal(status(), 'Saving…', 'waits for the newer settings');
    await elsewhere(fixture, {
      processing: {
        ...fixture.snapshot().settings.processing,
        smoothing_seconds: 0.5,
      },
    });
    await settle();
    const saved = fixture.snapshot().settings.processing;
    assert.equal(saved.shadow_lift_percent, 40);
    assert.equal(saved.smoothing_seconds, 0.5);
    assert.equal(field('processing.smoothing_seconds').value, '0.5');
    assert.equal(status(), 'Saved');
    assert(!text().includes('changed elsewhere'));
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
      message: 'The portal did not answer.',
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
    await key(mode('processed'), 'ArrowRight');
    assert.equal(fixture.snapshot().target, 'device_native');
    assert.equal(document.activeElement, mode('device_native'));
    assert.equal(mode('device_native').tabIndex, 0);
    assert.equal(mode('processed').tabIndex, -1);
    await key(mode('device_native'), 'ArrowLeft');
    assert.equal(fixture.snapshot().target, 'processed');
    assert.equal(document.activeElement, mode('processed'));
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
  assert.equal(
    normalizeWledAddress(' http://wled.local:8080/ '),
    'wled.local:8080',
  );
});

test('Settings keeps brightness and the profile choice on Home; the picture saves live', async () => {
  const profiles = [{ id: 'ps5', name: 'PS5' }];
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          profiles,
          active_profile: 'ps5',
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
    // Only black bars here: brightness and the profile choice live on Home.
    assert.equal(control('brightness_percent'), null);
    assert.equal(control('active_profile'), null);
    assert.equal(control('remove_black_bars').checked, false);
    assert.equal(valueOf('Colour profiles'), '1 profile · PS5 in use');
    await click(control('remove_black_bars'));
    await settle();
    assert.equal(saves(fixture), 1);
    assert.equal(
      fixture.snapshot().settings.processing.remove_black_bars,
      true,
    );

    await openPage('Fine-tuning');
    assert.deepEqual(
      [
        'shadow_lift_percent',
        'spatial_reach_percent',
        'smoothing_seconds',
        'sample_density',
      ].map((name) => attributes(control(name))),
      [
        ['range', '0', '0', '100', '1'],
        ['range', '10', '1', '100', '1'],
        // Smoothing and detail as sliders over the range worth tuning.
        ['range', '0', '0', '2', '0.05'],
        ['range', '1', '1', '512', '1'],
      ],
    );
    assert.deepEqual(
      ['shadow_lift_percent', 'spatial_reach_percent', 'sample_density'].map(
        (name) => control(name).labels[0].textContent,
      ),
      ['Shadow lift', 'Edge depth', 'Detail'],
    );

    // Two quick changes are applied once, after the user stops.
    await edit(control('spatial_reach_percent'), 30);
    await edit(control('spatial_reach_percent'), 20);
    assert.equal(saves(fixture), 1);
    await settle();
    assert.equal(saves(fixture), 2);

    await edit(control('sample_density'), 128);
    await edit(control('smoothing_seconds'), 0.5);
    assert.equal(
      $('output', control('smoothing_seconds').closest('.field')).textContent,
      '0.5\u00a0s',
    );
    await settle();
    assert.equal(saves(fixture), 3);
    assert.deepEqual(fixture.snapshot().settings.processing, {
      ...defaultProcessing,
      spatial_reach_percent: 20,
      sample_density: 128,
      smoothing_seconds: 0.5,
      remove_black_bars: true,
      profiles,
      active_profile: 'ps5',
    });

    // A field rejected by the server is marked and not retried until edited.
    fixture.failNext({
      kind: 'invalid',
      field: 'processing.shadow_lift_percent',
      message: 'must be 0-100',
    });
    await edit(control('shadow_lift_percent'), 30);
    await settle();
    assert.equal(
      errorOf(control('shadow_lift_percent')),
      'Prysm can’t use this value.',
    );
    await settle();
    assert.equal(saves(fixture), 4);
    await edit(control('shadow_lift_percent'), 20);
    await settle();
    assert.equal(saves(fixture), 5);
    assert.equal(
      fixture.snapshot().settings.processing.shadow_lift_percent,
      20,
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
    assert.deepEqual(
      ['capture.path', 'capture.width', 'capture.height', 'capture.fps'].map(
        (name) => field(name).value,
      ),
      ['/dev/video0', '640', '360', '25'],
    );
    assert.deepEqual(
      ['capture.width', 'capture.height', 'capture.fps'].map((name) => [
        field(name).min,
        field(name).max,
      ]),
      [
        ['1', '3840'],
        ['1', '2160'],
        ['1', '2147483647'],
      ],
    );
    // Nothing to save yet: only the way out.
    assert.equal(primary(), undefined);
    assert(button('Done', form('Video source')));

    await edit(field('capture.width'), 1280);
    assert.equal(primary().disabled, false);
    await click(button('Cancel', form('Video source')));
    assert.equal(window.location.hash, '#/settings');
    await openEditor('Video source');
    assert.equal(field('capture.width').value, '640', 'the draft was dropped');

    await edit(field('capture.width'), 1280);
    await edit(field('capture.height'), 720);
    assert.equal(saves(fixture), 0);
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings, {
      capture: {
        kind: 'device',
        path: '/dev/video0',
        fps: 25,
        width: 1280,
        height: 720,
      },
      output,
      processing: defaultProcessing,
    });
    // Screen sync is off: saved, and said not to be tried.
    assert.equal(
      status(),
      'Saved. Screen sync is off, so this source hasn’t been tried.',
    );

    // Out of range: shown at once, never sent, focused on the button.
    await edit(field('capture.width'), 4000);
    assert.equal(
      errorOf(field('capture.width')),
      'Enter a whole number from 1 to 3840.',
    );
    await submit(form('Video source'));
    assert.equal(saves(fixture), 1);
    assert.equal(status(), 'Not saved. Check the marked field.');
    assert.equal(document.activeElement, field('capture.width'));

    // An empty path is reported once a save is tried.
    await edit(field('capture.width'), 640);
    await edit(field('capture.path'), ' ');
    await submit(form('Video source'));
    assert.equal(errorOf(field('capture.path')), 'Enter a value.');
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
      message: 'height exceeds 2160',
    });
    await edit(field('capture.height'), 2000);
    await submit(form('Video source'));
    assert.equal(field('capture.height').getAttribute('aria-invalid'), 'true');
    // Prysm’s own words stay out: the field says how to fill it in.
    assert.equal(
      errorOf(field('capture.height')),
      'Use a size the device supports.',
    );
    assert.equal(document.activeElement, field('capture.height'));
    assert.equal(status(), 'Not saved. Check the marked field.');
    await edit(field('capture.height'), 1080);
    assert.equal(errorOf(field('capture.height')), undefined);
    assert.equal(field('capture.height').getAttribute('aria-invalid'), null);

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
    assert.equal(document.activeElement, field('output.address'));

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
    assert.equal(document.activeElement, $('[data-field="output.leds"]'));
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
    assert.equal(
      document.activeElement,
      $('[data-form-alert]', form('Your lights')),
    );
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

test('a section changed on another device: use theirs, or keep mine', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const conflict =
    'The video source was changed on another device since you opened this.';
  try {
    await openEditor('Video source');
    // Seen before saving: the draft is kept, and saving asks.
    await edit(field('capture.path'), '/dev/video1');
    await elsewhere(fixture, {
      capture: { kind: 'device', path: '/dev/video9' },
    });
    assert.equal(field('capture.path').value, '/dev/video1');
    await submit(form('Video source'));
    assert.equal(saves(fixture), 1, 'only the other device saved');
    assert.equal(status(), conflict);
    await click(button('Use theirs'));
    assert.equal(field('capture.path').value, '/dev/video9');

    // Refused by Prysm (409): the same choice; keeping mine saves it.
    await edit(field('capture.path'), '/dev/video2');
    fixture.failNext({
      kind: 'stale_revision',
      message: 'settings revision 1 is stale; current is 2',
      current: 2,
    });
    await submit(form('Video source'));
    assert.equal(status(), conflict);
    assert.equal(field('capture.path').value, '/dev/video2');
    await click(button('Keep mine'));
    assert.equal(fixture.snapshot().settings.capture.path, '/dev/video2');

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
    assert.equal(fixture.snapshot().settings.capture.path, '/dev/video2');
  } finally {
    await portal.unmount();
  }
});

/** The live problem line of a part of the setup, on Settings. */
const problem = (label) => $('.row-problem', row(label))?.textContent ?? '';

test('new lights are checked after saving; failing ones can be swapped back', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await press(mode('processed'));
    await openSettings();
    assert.equal(problem('Lights'), '', 'nothing wrong: an empty line, kept');
    await openEditor('Lights');
    await edit(field('output.address'), '192.0.2.99');
    await submit(form('Your lights'));
    assert.equal(fixture.snapshot().settings.output.address, '192.0.2.99');
    assert.equal(status(), 'Saved. Starting the lights…');
    assert(button('Go back to the previous lights'));

    // The new address does not answer: said plainly, with the way back.
    await act(async () =>
      fixture.publish({
        session: { state: 'stopped' },
        failure: {
          kind: 'adapter',
          section: 'output',
          message: 'no answer from 192.0.2.99',
        },
      }),
    );
    assert.equal(status(), 'Saved, but the lights are not responding.');
    assert(!text().includes('no answer from'), 'no raw message');

    // Settings shows the live problem on its row; the way back stays.
    await leaveEditor();
    assert.equal(problem('Lights'), 'The lights are not responding.');
    // The device stays named, before the problem.
    assert.equal(valueOf('Lights'), 'WLED · The lights are not responding.');
    assert.equal(problem('Video source'), '');
    await openEditor('Lights');
    assert.equal(status(), 'The lights are not responding.');
    await click(button('Go back to the previous lights'));
    assert.deepEqual(fixture.snapshot().settings.output, output);
    assert.equal(status(), 'Saved. Back to the previous lights.');
    assert.equal(field('output.address').value, '192.0.2.1');
    await leaveEditor();
    await act(async () =>
      fixture.publish({
        session: { state: 'running' },
        failure: null,
        input: { fps: 30, receiving: true },
      }),
    );
    assert.equal(problem('Lights'), '');

    // Working: flowing for a moment after the save says so.
    await openEditor('Video source');
    await edit(field('capture.path'), '/dev/video3');
    await submit(form('Video source'));
    assert.equal(status(), 'Saved. Waiting for the picture…');
    await act(async () => sleep(workingAfterMs + 100));
    assert.equal(status(), 'Saved. The picture is arriving.');
    assert.equal(button('Go back to the previous video source'), undefined);
    await click(button('Done'));
    assert.equal(window.location.hash, '#/settings');
  } finally {
    await portal.unmount();
  }
});

test('with screen sync off, a saved source can be tried at once', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await openEditor('Video source');
    await edit(field('capture.path'), '/dev/video4');
    await submit(form('Video source'));
    assert.equal(
      status(),
      'Saved. Screen sync is off, so this source hasn’t been tried.',
    );
    assert.equal(fixture.snapshot().target, 'off', 'saving starts nothing');
    await click(button('Turn on screen sync to try'));
    assert.equal(fixture.snapshot().target, 'processed');
    assert.equal(status(), 'Saved. Waiting for the picture…');
    await click(button('Go back to the previous video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, capture);
  } finally {
    await portal.unmount();
  }
});

test('a video file and no lights are developer options; a saved one still shows', async () => {
  const kinds = (name) =>
    $$('option', field(name)).map((option) => option.value);
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
    assert.equal(field('output.kind'), null);
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

test('kind selectors swap panels without losing drafts', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    // A video file and no lights are developer options.
    await act(async () => saveDeveloper(true));
    await openEditor('Video source');
    assert.deepEqual(
      $$('option', field('capture.kind')).map((item) => [
        item.value,
        item.textContent,
      ]),
      [
        ['device', 'Video capture device'],
        ['remote', 'Capture-only TV (remote)'],
        ['file', 'Video file (development only)'],
      ],
    );
    await edit(field('capture.kind'), 'remote');
    assert.equal(field('capture.path'), null);
    // The TV reaches this computer at the portal's address: no address field.
    assert.equal(field('capture.listen'), null);
    await edit(field('capture.kind'), 'device');
    assert.equal(field('capture.path').value, '/dev/video0');
    await edit(field('capture.kind'), 'file');
    assert.equal(field('capture.path').value, '');
    assert.equal(field('capture.path').labels[0].textContent, 'Video file');
    // Development only: it says what it needs.
    assert.match(
      form('Video source').textContent,
      /Plays a video file instead of a live picture, for testing\. Needs FFmpeg 9 or newer on this computer\./,
    );
    await edit(field('capture.kind'), 'remote');
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, { kind: 'remote' });
    assert.deepEqual(fixture.snapshot().settings.output, output);

    await leaveEditor();
    await openEditor('Lights');
    assert.deepEqual(
      $$('option', field('output.kind')).map((item) => item.value),
      ['wled', 'null'],
    );
    await edit(field('output.address'), 'wled.local');
    await edit(field('output.kind'), 'null');
    assert(
      form('Your lights').textContent.includes(
        'There is nothing to configure.',
      ),
    );
    await edit(field('output.kind'), 'wled');
    assert.equal(field('output.address').value, 'wled.local', 'kept');
    await edit(field('output.kind'), 'null');
    await submit(form('Your lights'));
    assert.deepEqual(fixture.snapshot().settings, {
      capture: { kind: 'remote' },
      output: { kind: 'null' },
      processing: defaultProcessing,
    });
    // Home: no WLED link, and these lights have no effects of their own.
    await leaveEditor();
    await click(button('Home'));
    assert.equal($('.device-link'), null);
    assert.equal(mode('device_native').disabled, true);
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
    assert.equal(field('capture.kind').value, 'device');
    assert.equal(errorOf(field('capture.path')), undefined);
    await submit(form('Video source'));
    assert.equal(errorOf(field('capture.path')), 'Enter a value.');
    assert.equal(saves(fixture), 0);
    await edit(field('capture.path'), ' /dev/video0 ');
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'device',
      path: '/dev/video0',
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
    assert.equal($('.total strong').textContent, '14');

    for (const edge of edges) await edit(field(`output.leds.${edge}`), 0);
    assert.equal(caption(), 'No LEDs yet.');
    await submit(form('Your lights'));
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'Use 1 to 16384 LEDs in total.',
    );
    assert.equal(document.activeElement, $('[data-field="output.leds"]'));
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
    await press(mode('device_native'));
    assert.equal($('.light-calm .device-link').href, 'http://wled.local:8080/');
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
        colour: { profile: 'warm' },
        processing: {
          ...snapshot.settings.processing,
          active_profile: 'ps5',
          profiles: [{ id: 'ps5' }],
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    // A video file and no lights are developer options.
    await act(async () => saveDeveloper(true));
    await openEditor('Video source');
    await edit(field('capture.fps'), 50);
    await submit(form('Video source'));
    await leaveEditor();
    await openEditor('Lights');
    await edit(field('output.leds.right'), 6);
    await submit(form('Your lights'));
    await leaveEditor();
    await openPage('Fine-tuning');
    await edit(field('processing.spatial_reach_percent'), 20);
    await settle();
    assert.deepEqual(fixture.snapshot().settings, {
      capture: { ...capture, future_option: 1, fps: 50 },
      output: { ...output, leds: { ...output.leds, right: 6 } },
      processing: {
        ...defaultProcessing,
        spatial_reach_percent: 20,
        active_profile: 'ps5',
        profiles: [{ id: 'ps5' }],
      },
      colour: { profile: 'warm' },
    });
  } finally {
    await portal.unmount();
  }
});

test('a size above 640 × 360 shows a quiet hint in both TV forms; it never blocks saving', async () => {
  const forms = [
    [WebosWebPortal, { kind: 'webos' }, largeSizeHint, /processor without/],
    [
      HeadlessWebPortal,
      { kind: 'remote' },
      largeStreamHint,
      /processor and more network without/,
    ],
  ];
  for (const [Host, start, hint, wording] of forms) {
    const portal = await renderPortal(Host, { capture: start, output });
    const { fixture } = portal;
    const input = (name) => field(`capture.${name}`);
    const shown = () => $('[data-size-hint]', form('Video source'));
    try {
      await openEditor('Video source');
      assert.equal(shown(), null, 'not at the default size');
      assert.match(hint, wording);
      assert.match(hint, /^640\u00a0×\u00a0360 is enough for the lights\./);
      for (const [width, height, expected] of [
        [640, 360, false],
        [320, 180, false],
        [1280, 720, true],
        [960, 540, true],
        [1280, 180, false],
      ]) {
        await edit(input('width'), width);
        await edit(input('height'), height);
        assert.equal(
          shown()?.textContent ?? null,
          expected ? hint : null,
          `${width}×${height}`,
        );
      }
      await edit(input('width'), 1280);
      await edit(input('height'), 720);
      const element = shown();
      assert.equal(element.className, 'help');
      assert.equal(element.getAttribute('role'), null, 'help, not an alert');
      assert.equal(errorOf(input('width')), undefined);
      assert.equal(errorOf(input('height')), undefined);
      await submit(form('Video source'));
      assert.deepEqual(
        [
          fixture.snapshot().settings.capture.width,
          fixture.snapshot().settings.capture.height,
        ],
        [1280, 720],
      );
      assert.equal(
        status(),
        'Saved. Screen sync is off, so this source hasn’t been tried.',
      );
      assert.equal(shown().textContent, hint, 'still shown after saving');
    } finally {
      await portal.unmount();
    }
  }
});

test('the webOS portal edits TV capture within the adapter limits', async () => {
  const portal = await renderPortal(WebosWebPortal, {
    capture: { kind: 'webos' },
    output,
  });
  const { fixture } = portal;
  const input = (name) => field(`capture.${name}`);
  try {
    await openEditor('Video source');
    assert.equal(field('capture.kind'), null);
    assert.equal($('.page-heading h2').textContent, 'Video source');
    assert.deepEqual(
      ['width', 'height', 'fps'].map((name) => [
        input(name).value,
        input(name).min,
        input(name).max,
        input(name).step,
      ]),
      [
        ['640', '2', '1920', '2'],
        ['360', '2', '1080', '2'],
        ['30', '1', '60', '1'],
      ],
    );
    assert.equal(input('include_ui').checked, true);
    await edit(input('width'), 1281);
    assert.equal(
      errorOf(input('width')),
      'Enter an even number from 2 to 1920.',
    );
    await edit(input('width'), 1280);
    await edit(input('height'), 720);
    await edit(input('fps'), 60);
    await click(input('include_ui'));
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'webos',
      width: 1280,
      height: 720,
      fps: 60,
      include_ui: false,
    });
    assert.deepEqual(fixture.snapshot().settings.output, output);
    await leaveEditor();
    await openEditor('Lights');
    assert.equal(field('output.kind'), null);
  } finally {
    await portal.unmount();
  }
});

test('the remote capture form asks the TV for a stream with the same picture fields as the webOS form', async () => {
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
    // Defaults shown for omitted fields; the shared fields' limits and labels.
    assert.deepEqual(
      ['width', 'height', 'fps'].map((name) => [
        input(name).value,
        input(name).min,
        input(name).max,
        input(name).step,
      ]),
      [
        ['320', '2', '1920', '2'],
        ['180', '2', '1080', '2'],
        ['30', '1', '60', '1'],
      ],
    );
    assert.deepEqual(
      ['width', 'height', 'fps', 'include_ui', 'encoding'].map(
        (name) => input(name).labels[0].textContent,
      ),
      [
        'Width (pixels)',
        'Height (pixels)',
        'Frame rate (fps)',
        'Include TV menus',
        'How the TV sends the picture',
      ],
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
    assert(button('Use this video source'));

    // Same validation as the webOS form.
    await edit(input('height'), 181);
    assert.equal(
      errorOf(input('height')),
      'Enter an even number from 2 to 1080.',
    );
    await edit(input('fps'), 61);
    assert.equal(errorOf(input('fps')), 'Enter a whole number from 1 to 60.');
    await edit(input('fps'), 25);
    // H.264 needs 16 × 16; raw does not.
    await edit(input('height'), 8);
    assert.equal(errorOf(input('encoding')), undefined);
    await edit(input('encoding'), 'h264_hardware');
    await submit(form('Video source'));
    assert.equal(saves(fixture), 0);
    assert.match(
      errorOf(input('encoding')),
      /A compressed picture needs at least 16 × 16 pixels; choose Uncompressed/,
    );
    await edit(input('height'), 144);
    await edit(input('width'), 256);
    await click(input('include_ui'));
    await edit(input('encoding'), 'h264_software');
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'remote',
      width: 256,
      height: 144,
      fps: 25,
      include_ui: false,
      encoding: 'h264_software',
    });
    assert.equal(
      status(),
      'Saved. Screen sync is off, so this source hasn’t been tried.',
    );

    // The server's own check on the encoding lands on the selector.
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.encoding',
      message: 'H.264 needs at least 16x16; use raw for smaller pictures',
    });
    await edit(input('encoding'), 'h264_hardware');
    await submit(form('Video source'));
    assert.equal(
      errorOf(input('encoding')),
      'Choose Uncompressed, or a picture of at least 16 × 16 pixels.',
    );
    assert.equal(document.activeElement, input('encoding'));
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
        target: 'processed',
        session: { state: 'stopped' },
        failure: { kind: 'adapter', message: refusal },
      }),
    );
    assert.equal($('.lighting-status').textContent, 'Screen sync stopped');
    assert.equal(
      $('.light-card [role="alert"]').textContent,
      'Screen sync could not start.',
    );
    assert.equal($('.light-details p').textContent, refusal, 'not shortened');
    // The video source editor says where the fix is.
    await openEditor('Video source');
    assert.match(
      form('Video source').textContent,
      /If the TV can’t send it, Home says why; then change how it sends the picture, or its size, here/,
    );
  } finally {
    await portal.unmount();
  }
});

test('limits and defaults written in the panels match the generated schemas', async () => {
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
  for (const [schema, limits, defaults] of [
    [application.Processing, processingLimits, {}],
    [application.CorrectionControls, correctionLimits, {}],
    [application.HueControls, hueLimits, {}],
    [desktop.DeviceSettings, deviceLimits, deviceDefaults],
    [desktop.FileSettings, fileLimits, fileDefaults],
    [webos.WebosSettings, tvPictureLimits, webosDefaults],
    [desktop.RemoteReceiverSettings, tvPictureLimits, remoteDefaults],
    [desktop.LedCounts, edges, {}],
    [webos.LedCounts, edges, {}],
  ]) {
    for (const [name, { min, max }] of Object.entries(limits)) {
      const property = schema.properties[name];
      assert.equal(min, property.minimum, name);
      // Where the schema has no maximum, the panel documents the type bound it uses.
      if (property.maximum !== undefined)
        assert.equal(max, property.maximum, name);
    }
    for (const [name, value] of Object.entries(defaults))
      assert.equal(value, schema.properties[name].default, name);
  }
  assert.equal(
    profileLimits.count,
    application.Processing.properties.profiles.maxItems,
  );
  assert.equal(
    profileLimits.name,
    application.ColorProfile.properties.name.maxLength,
  );
  for (const name of Object.keys(
    application.CorrectionControls.properties,
  ).filter((name) => name !== 'hue'))
    assert(name in correctionLimits, `${name} has a control`);
  for (const name of Object.keys(application.HueControls.properties))
    assert(name in hueLimits, `${name} has a control`);
  for (const schema of [webos.WebosSettings, desktop.RemoteReceiverSettings])
    for (const name of ['width', 'height'])
      assert.equal(
        schema.properties[name].multipleOf,
        tvPictureLimits[name].step,
        name,
      );
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
          profiles: [{ id: 'movie', name: 'Movie night' }],
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    assert.equal($$('form').length, 0, 'no forms on Home');
    // One headline, the lights' state; off, it says what Power will do.
    assert.equal($('.display'), null);
    assert.equal($('.light-calm').textContent, 'Power turns on Screen sync.');
    assert.equal($('.header-link').textContent, 'Settings');
    assert.equal($('.diagnostics'), null);
    assert.equal($('.preview'), null, 'no preview on Home');
    assert.deepEqual(
      $$('.light-card input').map((control) => control.name),
      ['processing.brightness_percent'],
    );
    // The setup card: the path from the picture through Prysm to the LEDs.
    assert.deepEqual(
      $$('.path-name').map((item) => item.textContent),
      ['Video capture device', 'Prysm', 'WLED'],
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
        'Colour profiles',
        'Fine-tuning',
        'Remove black bars',
        'Appearance',
        'About this Prysm',
        'Start over with setup',
      ],
    );
    assert.equal(
      valueOf('Video source'),
      'Video capture device · 640\u00a0×\u00a0360 · 30 fps',
    );
    assert.equal(valueOf('Lights'), 'WLED · 14 LEDs');
    assert.equal($('.settings a'), null, 'Open WLED stays on Home');
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
    assert.equal(window.location.hash, '');
    assert($('.light-card'));
    await act(async () =>
      fixture.publish({
        target: 'processed',
        session: { state: 'stopped' },
        failure: { kind: 'adapter', section: 'output', message: 'no answer' },
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
      { label: 'Elgato Cam Link 4K', fields: { path: id } },
      { label: 'FaceTime HD Camera', fields: { path: 'other' } },
    ],
    'output/wled': [
      { label: 'WLED-Gledopto', fields: { address: '192.0.2.1' } },
    ],
  });
  const portal = await renderPortal(
    HeadlessWebPortal,
    {
      capture: { kind: 'device', path: id, width: 640, height: 360, fps: 30 },
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
      // Nothing negative: without a profile the line stays empty.
      ['Prysm', '\u00a0'],
      // A non-breaking hyphen: never split at it.
      ['WLED\u2011Gledopto', '300 LEDs'],
    ]);
    assert(!$('.setup-card').textContent.includes(id), 'no raw device id');
    assert.equal($('.led-map-total'), null, 'the count is not repeated');
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
        target: 'processed',
        session: { state: 'running' },
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
    assert.equal($$('.path-stop')[2].querySelector('.path-figure'), null);
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
      'The lights show each frame about 91 ms after it was captured. The lights’ own delay is not included.',
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
        target: 'device_native',
        session: { state: 'native' },
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
        target: 'processed',
        failure: { kind: 'invalid', field: 'capture.width', message: 'x' },
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
        failure: {
          kind: 'adapter',
          section: 'capture',
          message:
            'video device capture failed: capture device does not support 640x400 at 30 Hz',
        },
      }),
    );
    assert.deepEqual(
      $$('.path-stop').map((stop) => stop.dataset.failed),
      ['true', 'false', 'false'],
    );
    assert.equal(info(), 'The video source is not working.');
    // The lights' side.
    await act(async () =>
      portal.fixture.publish({
        failure: { kind: 'adapter', section: 'output', message: 'x' },
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
    assert.equal(info(), 'The lights are not responding.');
    // One without a side: the path is broken at Prysm.
    await act(async () =>
      portal.fixture.publish({ failure: { kind: 'adapter', message: 'x' } }),
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
        target: 'processed',
        session: { state: 'running' },
        failure: null,
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
        session: { state: 'stopped' },
        failure: { kind: 'adapter', section: 'output', message: 'gone' },
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

test('discovered names are kept in this browser: shown at once next time, dropped for another device', async () => {
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
    assert.deepEqual(JSON.parse(memory[deviceNamesKey])['output:wled'], {
      fields: { address: '192.0.2.7' },
      name: 'WLED-Gledopto',
    });
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
  // Another device saved: not its name; the old entry goes.
  portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output: { ...output, address: '192.0.2.99' } },
    withStorage,
    { discovery: slow },
  );
  try {
    assert.equal(name(), 'WLED');
    assert.equal(JSON.parse(memory[deviceNamesKey])['output:wled'], undefined);
  } finally {
    await portal.unmount();
  }
});

test('colour profiles swap with one tap on Home, applied at once', async () => {
  const profiles = [
    { id: 'tv', name: 'TV' },
    { id: 'ps5', name: 'PS5' },
  ];
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          profiles,
          active_profile: 'tv',
        },
      },
    }),
  );
  const { fixture } = portal;
  const pressed = () =>
    $$('.chip')
      .filter((item) => item.getAttribute('aria-pressed') === 'true')
      .map((item) => item.textContent);
  try {
    assert.deepEqual(
      $$('.chip').map((item) => item.textContent),
      ['TV', 'PS5', 'No correction'],
    );
    assert.deepEqual(pressed(), ['TV']);

    await press(chip('ps5'));
    assert.deepEqual(pressed(), ['PS5']);
    assert.equal(saves(fixture), 1, 'no wait');
    assert.equal(fixture.snapshot().settings.processing.active_profile, 'ps5');
    assert.deepEqual(fixture.snapshot().settings.processing.profiles, profiles);

    await press(chip('ps5'));
    assert.equal(saves(fixture), 1, 'the active one again saves nothing');
    await press(chip('tv'));
    assert.equal(fixture.snapshot().settings.processing.active_profile, 'tv');
    await press(chip(''));
    assert.deepEqual(pressed(), ['No correction']);
    assert.equal(
      'active_profile' in fixture.snapshot().settings.processing,
      false,
    );
    assert.equal(saves(fixture), 3);

    // Profiles correct the screen's colours: quiet for the lights' effects.
    assert.equal($('.picture-controls').hasAttribute('inert'), false);
    await press(mode('device_native'));
    assert.equal($('.picture-controls').hasAttribute('inert'), true);
  } finally {
    await portal.unmount();
  }
  // Without profiles, Home offers to make one.
  const empty = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    assert.deepEqual(
      $$('.chip').map((item) => item.textContent),
      ['No correction', 'Calibrate colours'],
    );
    await press(button('Calibrate colours'));
    assert.equal(window.location.hash, '#/calibrate');
    await press(button('Leave'));
    assert.equal(window.location.hash, '');
  } finally {
    await empty.unmount();
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
    // An about:blank page has no storage: the choice still applies.
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
    await press(mode('processed'));
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
    await press(mode('processed'));
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
