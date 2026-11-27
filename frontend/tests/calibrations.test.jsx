import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import {
  newCalibrationId,
  processingDelayMs,
  calibrationLimits,
  undoForMs,
} from '@prysm/web-portal';
import {
  $,
  $$,
  button,
  click,
  edit,
  errorOf,
  field,
  navigatedOn,
  openPage,
  renderPortal,
  valueOf,
  sleep,
} from './dom';

const capture = { kind: 'device', device: '/dev/video0' };
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const uuid =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const unchanged = { hue: 0, strength: 0 };
const zero = {
  warmth: 0,
  tint: 0,
  midtones: 0,
  red: unchanged,
  green: unchanged,
  blue: unchanged,
};
const settle = () => act(async () => sleep(processingDelayMs + 100));
const saves = (fixture) =>
  fixture.calls.filter((call) => call === 'replaceSettings').length;
const processing = (fixture) => fixture.snapshot().settings.processing;
const enter = (element) =>
  act(async () =>
    element.dispatchEvent(
      new element.ownerDocument.defaultView.KeyboardEvent('keydown', {
        key: 'Enter',
        bubbles: true,
      }),
    ),
  );

test('calibrations are rows; each opens its page, saved live, deleted with Undo', async (t) => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          calibrations: [
            { id: 'movie', name: 'Movie night', correction: zero },
            { id: 'ps5', name: 'PS5', correction: zero },
          ],
          active_calibration: 'movie',
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    await openPage('Calibrations');
    assert.equal(window.location.hash, '#/settings/calibrations');
    assert.deepEqual(
      ['Movie night', 'PS5'].map((name) => valueOf(name)),
      ['In use', 'Not in use'],
    );
    assert.equal($('.save-status').textContent, 'All changes saved');

    await openPage('PS5');
    assert.equal(window.location.hash, '#/settings/calibrations/ps5');
    assert.equal($('.page-heading h2').textContent, 'PS5');
    const name = field('processing.calibrations[1].name');
    await edit(name, '');
    assert.equal(errorOf(name), 'Enter a name.');
    await edit(name, '  Game ');
    await settle();
    assert.equal(saves(fixture), 0);
    await enter(name);
    assert.equal(name.value, 'Game');
    await settle();
    assert.equal(processing(fixture).calibrations[1].name, 'Game');

    const inUse = field('processing.active_calibration');
    assert.equal(inUse.checked, false);
    await click(inUse);
    await settle();
    assert.equal(processing(fixture).active_calibration, 'ps5');
    const warmth = field('processing.calibrations[1].correction.warmth');
    assert.deepEqual(
      [warmth.type, warmth.min, warmth.max, warmth.step],
      ['range', '-100', '100', '1'],
    );
    await edit(warmth, 25);
    // Red, green and blue as in Calibrate: two sliders named by their ends.
    assert.deepEqual(
      $$('.calibration-page legend').map((legend) => legend.textContent),
      ['Red', 'Green', 'Blue'],
    );
    const blueHue = field('processing.calibrations[1].correction.blue.hue');
    assert.equal(
      blueHue.labels[0].textContent,
      'More turquoise or more purple',
    );
    await edit(blueHue, -10);
    await edit(field('processing.calibrations[1].correction.blue.strength'), 6);
    assert.equal(
      warmth.parentElement.querySelector('output').textContent,
      '+25',
    );
    await settle();
    assert.deepEqual(processing(fixture).calibrations[1].correction, {
      ...zero,
      warmth: 25,
      blue: { hue: -10, strength: 6 },
    });

    // Deleted from its page; the list keeps its row, with Undo.
    await click(button('Calibrations'));
    await openPage('Movie night');
    await click(button('Delete calibration'));
    await settle();
    assert.equal(window.location.hash, '#/settings/calibrations');
    assert.deepEqual(
      processing(fixture).calibrations.map((calibration) => calibration.name),
      ['Game'],
    );
    assert.equal(valueOf('Movie night'), 'Deleted. Undo');
    await click(button('Undo'));
    await settle();
    assert.deepEqual(
      processing(fixture).calibrations.map((calibration) => calibration.name),
      ['Movie night', 'Game'],
    );
    assert.equal(processing(fixture).active_calibration, 'ps5');
    assert.equal(valueOf('Movie night'), 'Not in use');

    await openPage('Game');
    t.mock.timers.enable({ apis: ['setTimeout'] });
    await click(button('Delete calibration'));
    await navigatedOn(t.mock.timers);
    assert.equal(window.location.hash, '#/settings/calibrations');
    assert.equal(processing(fixture).active_calibration, undefined);
    await act(async () => t.mock.timers.tick(undoForMs));
    assert(!button('Undo'));
    assert.equal(valueOf('Game'), 'Deleted. ');
    t.mock.timers.reset();
    await settle();

    // Removing the last calibration leaves the list out, as the server does.
    await openPage('Movie night');
    await click(button('Delete calibration'));
    await settle();
    assert.equal('calibrations' in processing(fixture), false);

    // New calibrations come from the Calibrate page.
    await openPage('New calibration');
    assert.equal(window.location.hash, '#/calibrate');
  } finally {
    await portal.unmount();
  }
});

test('calibration errors land on their control; the list is capped', async () => {
  const many = Array.from({ length: calibrationLimits.count }, (_, index) => ({
    id: `p${index}`,
    name: `Calibration ${index}`,
    correction: zero,
  }));
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          calibrations: many,
          active_calibration: 'p3',
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    await openPage('Calibrations');
    assert.equal(
      valueOf('New calibration'),
      'You have the most Prysm keeps (32). Delete one first.',
    );
    await openPage('New calibration');
    assert.equal(window.location.hash, '#/settings/calibrations', 'stays');

    await openPage('Calibration 3');
    fixture.failNext({
      kind: 'invalid',
      field: 'processing.calibrations[3].correction.tint',
      message: 'must be -100 to 100',
    });
    const tint = field('processing.calibrations[3].correction.tint');
    await edit(tint, 40);
    await settle();
    assert.equal(errorOf(tint), 'Prysm can’t use this value.');
    assert.equal($('.save-status').textContent, 'Not saved');
    await settle();
    assert.equal(saves(fixture), 1, 'not retried until edited');
    await edit(tint, 30);
    await settle();
    assert.equal(saves(fixture), 2);
    assert.equal(processing(fixture).calibrations[3].correction.tint, 30);
    assert.equal(errorOf(tint), undefined);

    await click(button('Delete calibration'));
    await settle();
    assert.equal(
      processing(fixture).calibrations.length,
      calibrationLimits.count - 1,
    );
    assert.equal(
      valueOf('New calibration'),
      'Test patterns on the TV, step by step',
    );
  } finally {
    await portal.unmount();
  }
});

test('new calibration ids are UUIDs no other calibration uses', () => {
  const id = newCalibrationId();
  assert.match(id, uuid);
  assert.notEqual(newCalibrationId([id]), id);
});
