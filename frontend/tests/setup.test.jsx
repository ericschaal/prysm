import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { saveDeveloper, staleNotice } from '@prysm/web-portal';
import { typedDeviceMs } from '@prysm/capture-device-ui';
import { defaultProcessing } from '../testkit/application';
import { fakeDiscoveryClient } from '../testkit/discovery';
import {
  $,
  $$,
  assertFocused,
  button,
  click,
  edit,
  errorOf,
  field,
  form,
  openEditor,
  openPage,
  openSettings,
  radio,
  renderPortal,
  sleep,
  submit,
  chooseKind,
  kindOf,
  kindRows,
  kinds,
} from './dom';

const capture = { kind: 'device', device: '/dev/video0' };
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const wizard = () => $('form.setup');
const heading = () => $('h2', wizard()).textContent;
const next = () => submit(wizard());
const saves = (fixture) =>
  fixture.calls.filter((call) => call === 'replaceSettings').length;
const plain = (element) => element.textContent.replace(/\u00a0/g, ' ');
const press = (element) =>
  act(async () => {
    element.click();
    await sleep(0);
  });

test('setup opens instead of the page while capture or lights are missing', async () => {
  for (const [initial, open] of [
    [{}, true],
    [{ capture }, true],
    [{ output }, true],
    [{ capture, output }, false],
  ]) {
    const portal = await renderPortal(HeadlessWebPortal, initial);
    try {
      assert.equal(!!wizard(), open, JSON.stringify(initial));
      assert.equal(!!$('.light-card'), !open);
      if (!open) {
        await openSettings();
        assert(
          $$('.row-label').some(
            (label) => label.textContent === 'Start over with setup',
          ),
        );
        continue;
      }
      assert.equal(heading(), 'Video source');
      assertFocused($('h2', wizard()));
      assert.equal($('[aria-current="step"]').textContent, 'Video source');
      // It starts from what is saved: a configured section is shown as it is.
      assert.equal(
        field('capture.device').value,
        initial.capture?.device ?? '',
      );
      await click(button('Leave setup'));
      assert(!wizard());
      assert.equal($('.light-card').tagName, 'SECTION');
      assert.equal(
        document.activeElement.textContent,
        'Setup closed; nothing was saved.',
      );
      assert.equal(button('Set up Prysm').disabled, false);
    } finally {
      await portal.unmount();
    }
  }
});

test('setup steps keep the drafts, check each step and save both sections with one request', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {});
  const { fixture } = portal;
  try {
    // The host's kinds and the kind's own editor; a video file only with
    // developer options on.
    assert.deepEqual(kinds('capture.kind'), ['device', 'remote']);
    await act(async () => saveDeveloper(true));
    assert.deepEqual(kinds('capture.kind'), ['device', 'remote', 'file']);
    // No device yet: nothing to choose a size for.
    assert(!$('.picture-choice', wizard()));
    await next();
    assert.equal(heading(), 'Video source');
    assert.equal(
      errorOf(field('capture.device')),
      'Choose a device, or enter its path.',
    );
    assertFocused(field('capture.device'));
    assert.equal(
      $('.form-status', wizard()).textContent,
      'Check the marked field to continue.',
    );

    await edit(field('capture.device'), ' /dev/video2 ');
    // The typed device is asked about once typing pauses.
    assert(!$('.picture-choice', wizard()));
    await act(async () => sleep(typedDeviceMs + 50));
    assert.equal(radio('Balanced').checked, true);
    await click(radio('Smooth'));
    await next();
    assert.equal(heading(), 'Your lights');
    assertFocused($('h2', wizard()));
    assert(!$('[role="alert"]', wizard()));
    await next();
    assert.equal(
      errorOf(field('output.address')),
      'Choose your lights, or enter their address.',
    );
    await edit(field('output.address'), 'http://wled.local/');
    await edit(field('output.leds.top'), 30);
    await edit(field('output.leds.left'), 12);

    await click(button('Back'));
    assert.equal(heading(), 'Video source');
    assert.equal(field('capture.device').value, ' /dev/video2 ');
    assert.equal(radio('Smooth').checked, true);
    await next();
    assert.equal(field('output.address').value, 'http://wled.local/');
    await next();
    assert.equal(heading(), 'Review and start');
    assert.deepEqual($$('.review-item p').map(plain), [
      'Capture card or camera/dev/video2, 640 × 360 at 60 fps',
      'WLEDwled.local, 42 LEDs',
    ]);
    await click(button('Change', $$('.review-item')[1]));
    assert.equal(heading(), 'Your lights');
    assert.equal(field('output.leds.top').value, '30');
    await next();
    assert.equal(saves(fixture), 0, 'nothing is saved before the end');

    await next();
    assert.equal(saves(fixture), 1);
    assert.deepEqual(fixture.snapshot().settings, {
      capture: {
        kind: 'device',
        device: '/dev/video2',
        width: 640,
        height: 360,
        fps: 60,
      },
      output: {
        kind: 'wled',
        address: 'wled.local',
        leds: { top: 30, left: 12 },
      },
      processing: defaultProcessing,
    });
    assert.deepEqual(fixture.calls.slice(-2), [
      'replaceSettings',
      'target:screen_sync',
    ]);
    assert(!wizard());
    assert.equal(
      document.activeElement.textContent,
      'Setup saved. Screen sync is starting.',
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Lights follow your screen',
    );
    await openSettings();
    assert(
      $$('.row-label').some(
        (label) => label.textContent === 'Start over with setup',
      ),
    );
  } finally {
    await portal.unmount();
  }
});

test('a server error sends setup back to the step with the field', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture });
  const { fixture } = portal;
  const review = async () => {
    for (let step = 0; step < 2 && heading() !== 'Review and start'; step++)
      await next();
    assert.equal(heading(), 'Review and start');
  };
  try {
    await next();
    await edit(field('output.address'), '224.0.0.1');
    await edit(field('output.leds.bottom'), 50);
    await review();

    fixture.failNext({
      kind: 'invalid',
      field: 'output.address',
      message: 'a unicast device address is required',
    });
    await next();
    assert.equal(heading(), 'Your lights');
    assert.equal(
      errorOf(field('output.address')),
      'Use the lights’ name, such as wled.local, or their number, such as 192.168.1.50.',
    );
    assertFocused(field('output.address'));
    assert.equal(
      $('.form-status', wizard()).textContent,
      'Not saved. Check the marked field.',
    );

    await edit(field('output.address'), '192.0.2.9');
    await review();
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.device',
      message: 'no such device',
    });
    await next();
    assert.equal(heading(), 'Video source');
    assert.equal(
      errorOf(field('capture.device')),
      'Choose a device, or enter its path.',
    );
    assertFocused(field('capture.device'));

    // An error without a control here stays on the review, in its alert.
    await review();
    fixture.failNext({
      kind: 'adapter',
      message: 'settings file is read-only',
    });
    await press(button('Save only'));
    assert.equal(heading(), 'Review and start');
    assertFocused($('[data-form-alert]', wizard()));
    assert.equal(
      document.activeElement.textContent,
      'Not saved: something went wrong. Try again.',
    );
    assert.equal(saves(fixture), 3);
    assert.equal(fixture.snapshot().revision, 0);
    assert.equal(fixture.snapshot().settings.output, undefined);
  } finally {
    await portal.unmount();
  }
});

test('setup again starts from the saved settings, and leaving it saves nothing', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await openPage('Start over with setup');
    assert.equal(heading(), 'Video source');
    assert.equal(kindOf('capture.kind'), 'device');
    assert.equal(field('capture.device').value, '/dev/video0');
    await edit(field('capture.device'), '/dev/video5');
    await next();
    assert.equal(field('output.address').value, '192.0.2.1');
    assert.equal(field('output.leds.left').value, '4');
    await click(button('Leave setup'));
    assert.equal(saves(fixture), 0);
    assert.equal(fixture.snapshot().settings.capture.device, '/dev/video0');
    assert($('.settings'), 'back in Settings');

    // Again: a fresh start from the saved values; Save only keeps lights off.
    await act(async () => saveDeveloper(true));
    await openPage('Start over with setup');
    assert.equal(field('capture.device').value, '/dev/video0');
    await next();
    await chooseKind('output.kind', 'null');
    await next();
    assert.deepEqual($$('.review-item p').map(plain), [
      'Capture card or camera/dev/video0, 640 × 360 at 30 fps',
      'No lights (test)',
    ]);
    await press(button('Save only'));
    assert.equal(saves(fixture), 1);
    assert.deepEqual(fixture.snapshot().settings.output, { kind: 'null' });
    assert.deepEqual(fixture.snapshot().settings.capture, capture);
    assert.equal(fixture.snapshot().target, 'off');
    assert(!fixture.calls.some((call) => call.startsWith('target:')));
    assert.equal(document.activeElement.textContent, 'Setup saved.');
  } finally {
    await portal.unmount();
  }
});

test('setup reloads instead of overwriting settings changed elsewhere', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { output });
  const { fixture } = portal;
  try {
    await edit(field('capture.device'), '/dev/video1');
    await act(async () => {
      const { settings, revision } = fixture.snapshot();
      await fixture.client.replaceSettings(revision, {
        ...settings,
        capture: { kind: 'file', path: '/clip.mkv' },
      });
      await sleep(0);
    });
    await next();
    await next();
    await next();
    assert.equal(saves(fixture), 1, 'only the other client saved');
    assert.equal(heading(), 'Review and start');
    assert.equal($('.form-status', wizard()).textContent, staleNotice);
    assert.match(
      plain($$('.review-item p')[0]),
      /^Video file \(development only\)\/clip\.mkv/,
    );
    await next();
    assert.equal(saves(fixture), 2);
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'file',
      path: '/clip.mkv',
    });
  } finally {
    await portal.unmount();
  }
});

test('the webOS portal sets up this TV without a kind choice', async () => {
  const portal = await renderPortal(WebosWebPortal, {});
  const { fixture } = portal;
  try {
    assert(!kindRows('capture.kind'));
    assert.equal(radio('Balanced').checked, true);
    assert(!field('capture.width'), 'no number fields');
    await next();
    assert(!kindRows('output.kind'));
    await edit(field('output.address'), '192.0.2.4');
    await edit(field('output.leds.top'), 20);
    await next();
    assert.deepEqual($$('.review-item p').map(plain), [
      'This TV640 × 360 at 30 fps, with menus',
      'WLED192.0.2.4, 20 LEDs',
    ]);
    await next();
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'webos',
      width: 640,
      height: 360,
      fps: 30,
      include_ui: true,
    });
    assert.equal(fixture.snapshot().target, 'screen_sync');
    await openEditor('Video source');
    assert.equal($('.page-heading h2').textContent, 'Video source');
    assert(form('Video source'));
  } finally {
    await portal.unmount();
  }
});

test('setup offers the same size-and-rate choice and saves the chosen preset', async () => {
  const portal = await renderPortal(WebosWebPortal, {});
  const { fixture } = portal;
  const label = (input) =>
    document.getElementById(input.getAttribute('aria-labelledby')).textContent;
  try {
    const rows = $$('input[type="radio"]', $('.picture-choice', wizard()));
    assert.deepEqual(rows.map(label), ['Balanced', 'Smooth', 'Efficient']);
    assert.equal(radio('Balanced').checked, true);
    await click(radio('Efficient'));
    await next();
    await edit(field('output.address'), '192.0.2.4');
    await edit(field('output.leds.top'), 20);
    await next();
    assert.equal(
      plain($$('.review-item p')[0]),
      'This TV320 × 180 at 30 fps, with menus',
    );
    await next();
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'webos',
      width: 320,
      height: 180,
      fps: 30,
      include_ui: true,
    });
  } finally {
    await portal.unmount();
  }
});

test('setup lists the devices and the lights present as rows as each step opens; the typed fields stay', async () => {
  const discovery = fakeDiscoveryClient({
    'capture/device': [
      {
        label: 'USB Video',
        detail: '/dev/video2',
        fields: { device: '/dev/video2' },
      },
      {
        label: 'Elgato',
        detail: '/dev/video4',
        fields: { device: '/dev/video4' },
      },
    ],
    'output/wled': [
      {
        label: 'Living room',
        detail: '192.0.2.40',
        led_count: 120,
        fields: { address: '192.0.2.40' },
      },
    ],
  });
  /** The line under the LED drawing that names the found lights' count. */
  const found = () => $('.leds > .field-note > .help');
  const portal = await renderPortal(HeadlessWebPortal, {}, undefined, {
    discovery: discovery.client,
  });
  const { fixture } = portal;
  try {
    // The devices present are looked for at once, none ticked, then the
    // row to type in, empty; the lights on their own step.
    assert.deepEqual(discovery.calls, ['capture/device']);
    const devices = () => $$('input[type="radio"]', $('.device-rows'));
    assert.deepEqual(
      devices().map((input) => plain(input.closest('label'))),
      ['USB Video', 'Elgato', ''],
    );
    assert.deepEqual(
      devices().map((input) => input.checked),
      [false, false, false],
    );
    assert.equal(field('capture.device').value, '');
    assert(!$('.picture-choice', wizard()), 'sizes follow the device');
    await click(devices()[1]);
    assert.equal(devices()[1].checked, true);
    assert.equal(radio('Balanced').checked, true);
    // Typing picks the typed row.
    await edit(field('capture.device'), '/dev/video9');
    assert.deepEqual(
      devices().map((input) => input.checked),
      [false, false, true],
    );
    await next();

    assert.deepEqual(discovery.calls, ['capture/device', 'output/wled']);
    const lights = () => $$('input[type="radio"]', $('.device-rows'));
    assert.deepEqual(
      lights().map((input) => plain(input.closest('label'))),
      ['Living room192.0.2.40, 120 LEDs', ''],
    );
    assert.equal(lights()[0].checked, false);
    await click(lights()[0]);
    assert.equal(lights()[0].checked, true);
    // Per-edge counts are left to the person; the found count is beside
    // them, amber while the edges add up to another.
    assert.equal(field('output.leds.top').value, '0');
    assert.equal(found().textContent, 'This WLED has 120 LEDs.');
    assert.equal(found().dataset.tone, 'warning');
    await edit(field('output.leds.top'), 120);
    assert.equal(found().textContent, 'This WLED has 120 LEDs.');
    assert.equal(found().dataset.tone, undefined, 'they agree');
    // Another address typed: the typed row is ticked, and the found count
    // no longer applies.
    await edit(field('output.address'), '192.0.2.41');
    assert.equal(found().textContent, '');
    assert.deepEqual(
      lights().map((input) => input.checked),
      [false, true],
    );
    // The found lights typed as a URL: still typed, the field kept as it is,
    // with their count.
    await edit(field('output.address'), 'http://192.0.2.40/');
    assert.equal(field('output.address').value, 'http://192.0.2.40/');
    assert.equal(lights()[1].checked, true);
    assert.equal(found().textContent, 'This WLED has 120 LEDs.');
    await edit(field('output.address'), '192.0.2.40');
    await next();
    await next();
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'device',
      device: '/dev/video9',
      width: 640,
      height: 360,
      fps: 30,
    });
    assert.deepEqual(fixture.snapshot().settings.output, {
      kind: 'wled',
      address: '192.0.2.40',
      leds: { top: 120 },
    });
  } finally {
    await portal.unmount();
  }
});

test('nothing found or a failed lookup leaves the typed row alone, ticked', async () => {
  const discovery = fakeDiscoveryClient();
  const portal = await renderPortal(WebosWebPortal, {}, undefined, {
    discovery: discovery.client,
  });
  const said = () => $('.device-rows [role="status"]').textContent;
  try {
    // This TV's capture has nothing to find.
    assert(!$('.device-rows'));
    await next();
    assert.equal(
      said(),
      'No WLED lights found on this network. Enter their address.',
    );
    const rows = () => $$('.device-rows .row-choice');
    assert.equal(rows().length, 1);
    assert.equal($('input', rows()[0]).checked, true);
    discovery.refuseNext({
      kind: 'unavailable',
      message: 'Prysm didn’t answer.',
    });
    await press(button('Look again'));
    assert.equal(said(), 'Couldn’t look for lights just now.');
    assert.equal(rows().length, 1);
    assert.equal(field('output.address').value, '');
  } finally {
    await portal.unmount();
  }
});

test('without a discovery client the editors offer only the typed row', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {});
  try {
    assert.equal($$('.device-rows .row-choice').length, 1);
    assert(!button('Look again'));
    assert.equal(field('capture.device').value, '');
  } finally {
    await portal.unmount();
  }
});
