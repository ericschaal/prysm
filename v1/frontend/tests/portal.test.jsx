import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import {
  correctionLimits,
  hueLimits,
  processingDelayMs,
  processingLimits,
  profileLimits,
  saveTheme,
  slowStartMs,
  staleNotice,
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
import {
  $,
  $$,
  button,
  click,
  edit,
  errorOf,
  field,
  form,
  openSettings,
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
    ? $('.light-info p').textContent
    : null;
const chip = (id) => $(`[data-profile="${id}"]`);
const hint = (element) =>
  document.getElementById(element.getAttribute('aria-describedby')).textContent;
const badge = (title) => $('.badge', form(title)).textContent;
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
    assert.equal($('.light-info').hasAttribute('inert'), true);
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
    assert.equal(info(), 'Waiting for video.');

    await act(async () =>
      fixture.publish({ input: { fps: 29.9, receiving: true } }),
    );
    assert.equal(info(), null);
    assert(
      !$('.light-card').textContent.includes('fps'),
      'no live frame rate on Home',
    );
    await openSettings();
    const rate = $$('.diagnostics dt').find(
      (term) => term.textContent === 'Capture rate',
    );
    assert.equal(rate.nextElementSibling.textContent, '30 fps');
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
    assert.deepEqual(
      $$('[data-target]').map((item) => item.textContent),
      ['Screen sync', 'WLED effects'],
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
    assert.equal(mode('processed').getAttribute('aria-pressed'), 'true');
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

    // The other tile switches mode while on; its own note replaces the
    // screen sync controls in the same place.
    await press(mode('device_native'));
    assert.deepEqual(targets().slice(-1), ['target:device_native']);
    assert.equal($('.lighting-status').textContent, 'WLED effects are on');
    assert.equal($('.light-card').dataset.mood, 'effects');
    const [syncPanel, effectsPanel] = $$('.mode-panel');
    assert.equal(syncPanel.hasAttribute('inert'), true);
    assert.equal(effectsPanel.hasAttribute('inert'), false);
    const link = $('.device-link', effectsPanel);
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
    assert.equal(info(), 'WLED at 192.0.2.1 did not switch to live mode');
    assert.equal(word(), 'Stopped');
    assert.equal(
      $('.light-card [role="alert"]').textContent,
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
    await act(async () => sleep(slowStartMs + 50));
    assert.equal(info(), 'Starting screen sync…');

    await act(async () => fixture.publish({ session: { state: 'running' } }));
    assert.equal(word(), 'On');
    assert.equal(info(), null);

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
    assert.equal(info(), null, 'still lit: nothing to say');
    await act(async () => fixture.publish({ session: { state: 'native' } }));
    assert.equal($('.light-frame').dataset.palette, 'effects');

    // Off: the light goes, and with it the word.
    await act(async () =>
      fixture.publish({ target: 'off', session: { state: 'stopped' } }),
    );
    assert.equal($('.light-card').dataset.lit, 'false');
    assert.equal(word(), 'Off');
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
    assert.equal(mode('device_native').textContent, 'Device effects');
    assert.equal(power().disabled, false);
    assert.equal($('.setup-links a'), null, 'no device to open');
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

test('picture controls show the state, enforce ranges and apply live', async () => {
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
    assert.deepEqual(
      [
        'brightness_percent',
        'shadow_lift_percent',
        'spatial_reach_percent',
        'smoothing_seconds',
        'sample_density',
      ].map((name) => attributes(control(name))),
      [
        ['range', '100', '0', '100', '1'],
        ['range', '0', '0', '100', '1'],
        ['range', '10', '1', '100', '1'],
        ['number', '0', '0', '', 'any'],
        ['number', '1', '1', '65535', '1'],
      ],
    );
    assert.equal(control('remove_black_bars').checked, false);
    assert.equal(
      control('brightness_percent').labels[0].textContent,
      'Brightness',
    );

    // Two quick changes are applied once, after the user stops.
    await edit(control('brightness_percent'), 90);
    await edit(control('brightness_percent'), 80);
    assert.equal($('.processing output').textContent, '80%');
    assert.equal(saves(fixture), 0);
    await settle();
    assert.equal(saves(fixture), 1);
    assert.deepEqual(fixture.snapshot().settings, {
      capture,
      output,
      processing: {
        ...defaultProcessing,
        brightness_percent: 80,
        profiles,
        active_profile: 'ps5',
      },
    });

    await edit(control('sample_density'), 0);
    assert.equal(
      control('sample_density').getAttribute('aria-invalid'),
      'true',
    );
    assert.equal(
      errorOf(control('sample_density')),
      'Enter a whole number from 1 to 65535.',
    );
    await edit(control('smoothing_seconds'), -1);
    assert.equal(
      errorOf(control('smoothing_seconds')),
      'Enter a number of at least 0.',
    );
    await settle();
    assert.equal(saves(fixture), 1);

    await edit(control('sample_density'), 128);
    await edit(control('smoothing_seconds'), 0.5);
    await click(control('remove_black_bars'));
    assert.equal(errorOf(control('sample_density')), undefined);
    await settle();
    assert.equal(saves(fixture), 2);
    assert.deepEqual(fixture.snapshot().settings.processing, {
      ...defaultProcessing,
      brightness_percent: 80,
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
    assert.equal(errorOf(control('shadow_lift_percent')), 'must be 0-100');
    await settle();
    assert.equal(saves(fixture), 3);
    await edit(control('shadow_lift_percent'), 20);
    await settle();
    assert.equal(saves(fixture), 4);
    assert.equal(
      fixture.snapshot().settings.processing.shadow_lift_percent,
      20,
    );
  } finally {
    await portal.unmount();
  }
});

test('the capture form saves the exact section on Save, with dirty state and reset', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { ...capture, fps: 25 },
    output,
  });
  const { fixture } = portal;
  try {
    await openSettings();
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
    assert.equal(badge('Video source'), 'Saved');
    assert.equal(button('Save', form('Video source')).disabled, true);

    await edit(field('capture.width'), 1280);
    assert.equal(badge('Video source'), 'Unsaved changes');
    await click(button('Reset', form('Video source')));
    assert.equal(field('capture.width').value, '640');
    assert.equal(badge('Video source'), 'Saved');

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
    assert.equal(badge('Video source'), 'Saved');

    // Out of range: shown at once, never sent, focused on Save.
    await edit(field('capture.width'), 4000);
    assert.equal(
      errorOf(field('capture.width')),
      'Enter a whole number from 1 to 3840.',
    );
    await submit(form('Video source'));
    assert.equal(saves(fixture), 1);
    assert.equal(
      $('[data-form-alert]', form('Video source')).textContent,
      'Correct the highlighted fields before saving.',
    );
    assert.equal(document.activeElement, field('capture.width'));

    // An empty path is reported once a save is tried.
    await click(button('Reset', form('Video source')));
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
    await openSettings();
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.height',
      message: 'height exceeds 2160',
    });
    await edit(field('capture.height'), 2000);
    await submit(form('Video source'));
    assert.equal(field('capture.height').getAttribute('aria-invalid'), 'true');
    assert.equal(errorOf(field('capture.height')), 'height exceeds 2160');
    assert.equal(document.activeElement, field('capture.height'));
    assert.equal(
      $('[role="alert"]', form('Video source')).textContent,
      'Not saved: height exceeds 2160',
    );
    await edit(field('capture.height'), 1080);
    assert.equal(errorOf(field('capture.height')), undefined);
    assert.equal(field('capture.height').getAttribute('aria-invalid'), null);

    fixture.failNext({
      kind: 'invalid',
      field: 'output.address',
      message: 'a unicast device address is required',
    });
    await edit(field('output.address'), '224.0.0.1');
    await submit(form('Lights'));
    assert.equal(
      errorOf(field('output.address')),
      'a unicast device address is required',
    );
    assert.equal(document.activeElement, field('output.address'));

    fixture.failNext({
      kind: 'invalid',
      field: 'output.leds',
      message: 'total LED count must be between 1 and 65535',
    });
    await edit(field('output.address'), '192.0.2.7');
    await submit(form('Lights'));
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'total LED count must be between 1 and 65535',
    );
    assert.equal(document.activeElement, $('[data-field="output.leds"]'));
    // Editing any edge answers the group's error.
    await edit(field('output.leds.right'), 2);
    assert.equal($('[data-field-message="output.leds"]'), null);

    // A field this form does not show still explains itself.
    fixture.failNext({
      kind: 'invalid',
      field: 'output.unknown',
      message: 'unknown field',
    });
    await submit(form('Lights'));
    assert.equal(
      document.activeElement,
      $('[data-form-alert]', form('Lights')),
    );
    assert.equal(
      document.activeElement.textContent,
      'Not saved: unknown field',
    );
    // Every attempt reached the server and was refused; nothing was saved.
    assert.equal(saves(fixture), 4);
    assert.equal(fixture.snapshot().revision, 0);
  } finally {
    await portal.unmount();
  }
});

test('a section changed elsewhere is reloaded instead of overwritten', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await openSettings();
    // Seen before saving: the draft is kept until Save, which reloads.
    await edit(field('capture.path'), '/dev/video1');
    await elsewhere(fixture, {
      capture: { kind: 'device', path: '/dev/video9' },
    });
    assert(text().includes('Changed elsewhere since you started editing.'));
    assert.equal(field('capture.path').value, '/dev/video1');
    await submit(form('Video source'));
    assert.equal(saves(fixture), 1);
    assert.equal(field('capture.path').value, '/dev/video9');
    assert.equal(
      $('.notice[role="status"]', form('Video source')).textContent,
      staleNotice,
    );

    // Refused by the server (409): reloaded the same way.
    await edit(field('capture.path'), '/dev/video2');
    fixture.failNext({
      kind: 'stale_revision',
      message: 'settings revision 1 is stale; current is 2',
      current: 2,
    });
    await submit(form('Video source'));
    assert.equal(field('capture.path').value, '/dev/video9');
    assert.equal(
      $('.notice[role="status"]', form('Video source')).textContent,
      staleNotice,
    );
    assert.equal(fixture.snapshot().settings.capture.path, '/dev/video9');

    // Clean forms follow other clients.
    await elsewhere(fixture, { output: { ...output, address: '192.0.2.50' } });
    assert.equal(field('output.address').value, '192.0.2.50');

    // Saving other sections (here a live picture change) is not a conflict.
    await edit(field('capture.path'), '/dev/video3');
    await edit(field('processing.brightness_percent'), 50);
    await settle();
    await submit(form('Video source'));
    assert.equal(
      $('.notice[role="status"]', form('Video source')).textContent,
      'Saved.',
    );
    assert.equal(fixture.snapshot().settings.capture.path, '/dev/video3');
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 50);
    assert.equal(fixture.snapshot().settings.output.address, '192.0.2.50');
  } finally {
    await portal.unmount();
  }
});

test('kind selectors swap panels without losing drafts or the other section', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await openSettings();
    assert.deepEqual(
      $$('option', field('capture.kind')).map((item) => [
        item.value,
        item.textContent,
      ]),
      [
        ['device', 'Video capture device'],
        ['remote', 'Capture-only TV (remote)'],
        ['file', 'Video file'],
      ],
    );
    assert.deepEqual(
      $$('option', field('output.kind')).map((item) => item.value),
      ['wled', 'null'],
    );

    await edit(field('output.address'), 'wled.local');
    await edit(field('capture.kind'), 'remote');
    assert.equal(field('capture.path'), null);
    assert.equal(field('capture.listen').value, '');
    await edit(field('capture.listen'), '0.0.0.0:7380');
    await edit(field('capture.kind'), 'device');
    assert.equal(field('capture.path').value, '/dev/video0');
    await edit(field('capture.kind'), 'file');
    assert.equal(field('capture.path').value, '');
    assert.equal(field('capture.path').labels[0].textContent, 'Video file');
    await edit(field('capture.kind'), 'remote');
    assert.equal(field('capture.listen').value, '0.0.0.0:7380');
    assert.equal(field('output.address').value, 'wled.local');

    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'remote',
      listen: '0.0.0.0:7380',
    });
    assert.deepEqual(fixture.snapshot().settings.output, output);
    assert.equal(field('output.address').value, 'wled.local');
    assert.equal(badge('Lights'), 'Unsaved changes');

    await click(button('Reset', form('Lights')));
    await edit(field('output.kind'), 'null');
    assert(
      form('Lights').textContent.includes('There is nothing to configure.'),
    );
    await submit(form('Lights'));
    assert.deepEqual(fixture.snapshot().settings, {
      capture: { kind: 'remote', listen: '0.0.0.0:7380' },
      output: { kind: 'null' },
      processing: defaultProcessing,
    });
    // Home: no WLED link, and these lights have no effects of their own.
    await click(button('Home'));
    assert.equal($('.setup-links a'), null);
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
    assert.equal(badge('Video source'), 'Not configured');
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
    await openSettings();
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
      ['0', '65535', '1'],
    );
    assert.equal(
      caption(),
      'LED 1 is at the bottom-left corner. Order: left upward: LEDs 1–4; top rightward: LEDs 5–14; right: none; bottom: none.',
    );
    assert.equal($('.total strong').textContent, '14');

    for (const edge of edges) await edit(field(`output.leds.${edge}`), 0);
    assert.equal(caption(), 'No LEDs yet.');
    await submit(form('Lights'));
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'The total must be 1 to 65535 LEDs. Use 0 for an edge without LEDs.',
    );
    assert.equal(document.activeElement, $('[data-field="output.leds"]'));
    await edit(field('output.leds.top'), 70000);
    assert.equal(
      errorOf(field('output.leds.top')),
      'Enter a whole number from 0 to 65535.',
    );
    await edit(field('output.leds.top'), 65535);
    await edit(field('output.leds.left'), 1);
    assert(text().includes('The total must be 1 to 65535 LEDs.'));
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
    await submit(form('Lights'));
    assert.deepEqual(fixture.snapshot().settings.output, {
      kind: 'wled',
      address: 'wled.local:8080',
      leds: { top: 96, right: 54, bottom: 96, left: 0 },
    });
    assert.equal(field('output.address').value, 'wled.local:8080');
    await click(button('Home'));
    assert.equal($('.setup-links a').href, 'http://wled.local:8080/');
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
    await openSettings();
    await edit(field('capture.fps'), 50);
    await submit(form('Video source'));
    await edit(field('output.leds.right'), 6);
    await submit(form('Lights'));
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
      { kind: 'remote', listen: '0.0.0.0:7380' },
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
      await openSettings();
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
      assert.equal(badge('Video source'), 'Saved');
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
    await openSettings();
    assert.equal(field('capture.kind'), null);
    assert.equal(field('output.kind'), null);
    assert.equal(
      form('Video source').querySelector('h2').textContent,
      'Video source',
    );
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
  } finally {
    await portal.unmount();
  }
});

test('the remote capture form asks the TV for a stream with the same picture fields as the webOS form', async () => {
  const listen = '0.0.0.0:7380';
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'remote', listen },
    output,
  });
  const { fixture } = portal;
  const input = (name) => field(`capture.${name}`);
  const help = (element) =>
    element.closest('.field').querySelector('.help')?.textContent;
  try {
    await openSettings();
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
        'Stream encoding',
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
        ['h264_hardware', 'Hardware H.264 (recommended)'],
        ['h264_software', 'Software H.264'],
        ['raw', 'Uncompressed'],
      ],
    );
    assert.match(
      help(input('encoding')),
      /Least work for the TV, little network \(about 2\sMbit\/s\)/,
    );
    await edit(input('encoding'), 'h264_software');
    assert.match(
      help(input('encoding')),
      /More work for the TV.*same little network/,
    );
    await edit(input('encoding'), 'raw');
    assert.match(help(input('encoding')), /ten times the network/);
    assert.equal(badge('Video source'), 'Unsaved changes');

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
      /H\.264 needs at least 16 × 16 pixels; choose Uncompressed/,
    );
    await edit(input('height'), 144);
    await edit(input('width'), 256);
    await click(input('include_ui'));
    await edit(input('encoding'), 'h264_software');
    await submit(form('Video source'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'remote',
      listen,
      width: 256,
      height: 144,
      fps: 25,
      include_ui: false,
      encoding: 'h264_software',
    });
    assert.equal(badge('Video source'), 'Saved');

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
      'H.264 needs at least 16x16; use raw for smaller pictures',
    );
    assert.equal(document.activeElement, input('encoding'));
  } finally {
    await portal.unmount();
  }
});

test('the sender refusing the requested stream is shown in full with the lighting state', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {
    capture: { kind: 'remote', listen: '0.0.0.0:7380' },
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
      refusal,
      'not shortened',
    );
    // The capture form, in Settings, says where the fix is.
    await openSettings();
    assert.match(
      form('Video source').textContent,
      /If the TV cannot send it, lighting stops and says why; change the encoding or size here/,
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
    assert.equal($('.diagnostics'), null);
    assert.equal($('.preview'), null, 'no preview on Home');
    assert.deepEqual(
      $$('.light-card input').map((control) => control.name),
      ['processing.brightness_percent'],
    );
    // The setup card: what sends the picture, what shows it, the LEDs.
    assert.deepEqual(
      $$('.setup-flow strong').map((item) => item.textContent),
      ['Video capture device', 'WLED'],
    );
    assert.equal(
      $('.led-map figcaption').textContent,
      '14 LEDs: 10 along the top, 0 on the right, 0 along the bottom and 4 on the left.',
    );
    await edit(field('processing.brightness_percent'), 55);
    await settle();
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 55);

    await click(button('Change setup'));
    assert.equal(window.location.hash, '#/settings');
    await click(button('Home'));
    await openSettings();
    assert.equal(window.location.hash, '#/settings');
    assert.equal(document.activeElement.textContent, 'Settings');
    assert(form('Video source') && form('Lights'));
    assert($('.diagnostics'));
    await click(button('Home'));
    assert.equal(window.location.hash, '');
    assert($('.light-card'));
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

    // Profiles correct the screen's colours: folded away for the lights' effects.
    assert.equal($('.mode-panel').hasAttribute('inert'), false);
    await press(mode('device_native'));
    assert.equal($('.mode-panel').hasAttribute('inert'), true);
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
    const choice = (label) =>
      $$('.theme-choice input').find(
        (input) => input.parentElement.textContent === label,
      );
    assert.equal(choice('System').checked, true);
    // An about:blank page has no storage: the choice still applies.
    await click(choice('Dark'));
    assert.equal(document.documentElement.getAttribute('data-theme'), 'dark');
    assert.equal(choice('Dark').checked, true);
    assert.equal(storedTheme(), 'system', 'nothing kept without storage');
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
