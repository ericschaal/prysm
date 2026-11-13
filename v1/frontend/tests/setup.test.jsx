import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { staleNotice } from '@prysm/web-portal';
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
  openSettings,
  renderPortal,
  sleep,
  submit,
} from './dom';

const capture = { kind: 'device', path: '/dev/video0' };
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
        assert.equal(button('Run setup again').disabled, false);
        continue;
      }
      assert.equal(heading(), 'Video source');
      assert.equal(document.activeElement, $('h2', wizard()));
      assert.equal($('[aria-current="step"]').textContent, 'Video source');
      // It starts from what is saved: a configured section is shown as it is.
      assert.equal(field('capture.path').value, initial.capture?.path ?? '');
      await click(button('Leave setup'));
      assert.equal(wizard(), null);
      assert.equal($('.light-card').tagName, 'SECTION');
      assert.equal(
        document.activeElement.textContent,
        'Setup closed; nothing was saved.',
      );
      // Home says what is missing and offers setup.
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
    // The host's kinds and the kind's own editor.
    assert.deepEqual(
      $$('option', field('capture.kind')).map((item) => item.value),
      ['device', 'remote', 'file'],
    );
    await next();
    assert.equal(heading(), 'Video source');
    assert.equal(errorOf(field('capture.path')), 'Enter a value.');
    assert.equal(document.activeElement, field('capture.path'));
    assert.equal(
      $('[role="alert"]', wizard()).textContent,
      'Correct the highlighted fields to continue.',
    );

    await edit(field('capture.path'), ' /dev/video2 ');
    await edit(field('capture.width'), 1280);
    await next();
    assert.equal(heading(), 'Your lights');
    assert.equal(document.activeElement, $('h2', wizard()));
    assert.equal($('[role="alert"]', wizard()), null);
    await next();
    assert.equal(errorOf(field('output.address')), 'Enter a value.');
    await edit(field('output.address'), 'http://wled.local/');
    await edit(field('output.leds.top'), 30);
    await edit(field('output.leds.left'), 12);

    await click(button('Back'));
    assert.equal(heading(), 'Video source');
    assert.equal(field('capture.path').value, ' /dev/video2 ');
    assert.equal(field('capture.width').value, '1280');
    await next();
    assert.equal(field('output.address').value, 'http://wled.local/');
    await next();
    assert.equal(heading(), 'Review and start');
    assert.deepEqual($$('.review-item p').map(plain), [
      'Video capture device/dev/video2, 1280 × 360 at 30 fps',
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
      capture: { kind: 'device', path: '/dev/video2', width: 1280 },
      output: {
        kind: 'wled',
        address: 'wled.local',
        leds: { top: 30, left: 12 },
      },
      processing: defaultProcessing,
    });
    assert.deepEqual(fixture.calls.slice(-2), [
      'replaceSettings',
      'target:processed',
    ]);
    assert.equal(wizard(), null);
    assert.equal(
      document.activeElement.textContent,
      'Setup saved. Screen sync is starting.',
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Lights follow your screen',
    );
    await openSettings();
    assert.equal(button('Run setup again').disabled, false);
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
      'a unicast device address is required',
    );
    assert.equal(document.activeElement, field('output.address'));
    assert.equal(
      $('[role="alert"]', wizard()).textContent,
      'Not saved: a unicast device address is required',
    );

    await edit(field('output.address'), '192.0.2.9');
    await review();
    fixture.failNext({
      kind: 'invalid',
      field: 'capture.path',
      message: 'no such device',
    });
    await next();
    assert.equal(heading(), 'Video source');
    assert.equal(errorOf(field('capture.path')), 'no such device');
    assert.equal(document.activeElement, field('capture.path'));

    // An error without a control here stays on the review, in its alert.
    await review();
    fixture.failNext({
      kind: 'adapter',
      message: 'settings file is read-only',
    });
    await press(button('Save only'));
    assert.equal(heading(), 'Review and start');
    assert.equal(document.activeElement, $('[data-form-alert]', wizard()));
    assert.equal(
      document.activeElement.textContent,
      'Not saved: settings file is read-only',
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
    await openSettings();
    await click(button('Run setup again'));
    assert.equal(heading(), 'Video source');
    assert.equal(field('capture.kind').value, 'device');
    assert.equal(field('capture.path').value, '/dev/video0');
    await edit(field('capture.path'), '/dev/video5');
    await next();
    assert.equal(field('output.address').value, '192.0.2.1');
    assert.equal(field('output.leds.left').value, '4');
    await click(button('Leave setup'));
    assert.equal(saves(fixture), 0);
    assert.equal(field('capture.path').value, '/dev/video0');

    // Again: a fresh start from the saved values; Save only keeps lights off.
    await click(button('Run setup again'));
    assert.equal(field('capture.path').value, '/dev/video0');
    await next();
    await edit(field('output.kind'), 'null');
    await next();
    assert.deepEqual($$('.review-item p').map(plain), [
      'Video capture device/dev/video0, 640 × 360 at 30 fps',
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
    await edit(field('capture.path'), '/dev/video1');
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
    assert.equal(
      $('.notice[role="status"]', wizard()).textContent,
      staleNotice,
    );
    assert.match(plain($$('.review-item p')[0]), /^Video file\/clip\.mkv/);
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
    assert.equal(field('capture.kind'), null);
    assert.equal(field('capture.width').value, '640');
    await next();
    assert.equal(field('output.kind'), null);
    await edit(field('output.address'), '192.0.2.4');
    await edit(field('output.leds.top'), 20);
    await next();
    assert.deepEqual($$('.review-item p').map(plain), [
      'This TV640 × 360 at 30 fps, with menus',
      'WLED192.0.2.4, 20 LEDs',
    ]);
    await next();
    assert.deepEqual(fixture.snapshot().settings.capture, { kind: 'webos' });
    assert.equal(fixture.snapshot().target, 'processed');
    await openSettings();
    assert.equal(
      form('Video source').querySelector('h2').textContent,
      'Video source',
    );
  } finally {
    await portal.unmount();
  }
});

test('setup shows the same quiet size hint above 640 × 360', async () => {
  const portal = await renderPortal(WebosWebPortal, {});
  try {
    assert.equal($('[data-size-hint]', wizard()), null);
    await edit(field('capture.width'), 1280);
    await edit(field('capture.height'), 720);
    assert.match(
      $('[data-size-hint]', wizard()).textContent,
      /is enough for the lights/,
    );
    await next();
    assert.equal(
      field('output.kind'),
      null,
      'the hint does not stop the next step',
    );
  } finally {
    await portal.unmount();
  }
});

test('setup offers found devices and lights; the typed fields stay', async () => {
  const discovery = fakeDiscoveryClient({
    'capture/device': [
      {
        label: 'USB Video',
        detail: '/dev/video2',
        fields: { path: '/dev/video2' },
      },
      {
        label: 'Elgato',
        detail: '/dev/video4',
        fields: { path: '/dev/video4' },
      },
    ],
    'output/wled': [
      {
        label: 'Living room',
        detail: '192.0.2.40, 120 LEDs',
        fields: { address: '192.0.2.40' },
      },
    ],
  });
  const portal = await renderPortal(HeadlessWebPortal, {}, undefined, {
    discovery: discovery.client,
  });
  const { fixture } = portal;
  try {
    // Nothing is looked up until asked.
    assert.deepEqual(discovery.calls, []);
    await press(button('Find devices'));
    assert.deepEqual(discovery.calls, ['capture/device']);
    assert.deepEqual($$('.found button').map(plain), [
      'USB Video/dev/video2',
      'Elgato/dev/video4',
    ]);
    await press($$('.found button')[1]);
    assert.equal(field('capture.path').value, '/dev/video4');
    assert.equal($$('.found button')[1].getAttribute('aria-pressed'), 'true');
    // Typing still works after picking.
    await edit(field('capture.path'), '/dev/video9');
    await next();

    await press(button('Find lights'));
    assert.deepEqual(discovery.calls, ['capture/device', 'output/wled']);
    assert.equal(plain($('.found button')), 'Living room192.0.2.40, 120 LEDs');
    await press($('.found button'));
    assert.equal(field('output.address').value, '192.0.2.40');
    // Per-edge counts are left to the person.
    assert.equal(field('output.leds.top').value, '0');
    await edit(field('output.leds.top'), 120);
    await next();
    await next();
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'device',
      path: '/dev/video9',
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

test('nothing found or a failed lookup leaves the typed field', async () => {
  const discovery = fakeDiscoveryClient();
  const portal = await renderPortal(WebosWebPortal, {}, undefined, {
    discovery: discovery.client,
  });
  try {
    // This TV's capture has nothing to find: no button.
    assert.equal(button('Find devices'), undefined);
    await next();
    await press(button('Find lights'));
    assert.equal(
      $('.find [role="status"]').textContent,
      'No WLED lights found on this network. Enter the address below.',
    );
    discovery.refuseNext({
      kind: 'unavailable',
      message: 'The portal did not answer.',
    });
    await press(button('Find lights'));
    assert.equal(
      $('.find [role="status"]').textContent,
      'Could not look: The portal did not answer. Enter the address below.',
    );
    assert.equal(field('output.address').value, '');
  } finally {
    await portal.unmount();
  }
});

test('without a discovery client the editors offer only typed fields', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {});
  try {
    assert.equal(button('Find devices'), undefined);
    assert.equal(field('capture.path').value, '');
  } finally {
    await portal.unmount();
  }
});
