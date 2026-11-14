import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import {
  newProfileId,
  processingDelayMs,
  profileLimits,
  undoForMs,
} from '@prysm/web-portal';
import {
  $,
  button,
  click,
  edit,
  errorOf,
  field,
  openPage,
  renderPortal,
  valueOf,
  sleep,
} from './dom';

const capture = { kind: 'device', path: '/dev/video0' };
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const uuid =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const zero = {
  warmth: 0,
  tint: 0,
  midtones: 0,
  intensity: 0,
  hue: { red: 0, yellow: 0, green: 0, cyan: 0, blue: 0, magenta: 0 },
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

test('colour profiles are rows; each opens its page, saved live, deleted with Undo', async (t) => {
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture, output },
    (snapshot) => ({
      settings: {
        ...snapshot.settings,
        processing: {
          ...snapshot.settings.processing,
          profiles: [
            { id: 'movie', name: 'Movie night', correction: zero },
            { id: 'ps5', name: 'PS5' },
          ],
          active_profile: 'movie',
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    await openPage('Colour profiles');
    assert.equal(window.location.hash, '#/settings/profiles');
    assert.deepEqual(
      ['Movie night', 'PS5'].map((name) => valueOf(name)),
      ['In use', 'Not in use'],
    );
    assert.equal($('.save-status').textContent, 'All changes saved');

    // A profile's page: its name, whether it is in use, its correction.
    await openPage('PS5');
    assert.equal(window.location.hash, '#/settings/profiles/ps5');
    assert.equal($('.page-heading h2').textContent, 'PS5');
    const name = field('processing.profiles[1].name');
    await edit(name, '');
    assert.equal(errorOf(name), 'Enter a name.');
    await edit(name, '  Game ');
    await settle();
    assert.equal(saves(fixture), 0);
    await enter(name);
    assert.equal(name.value, 'Game');
    await settle();
    assert.equal(processing(fixture).profiles[1].name, 'Game');

    const inUse = field('processing.active_profile');
    assert.equal(inUse.checked, false);
    await click(inUse);
    await settle();
    assert.equal(processing(fixture).active_profile, 'ps5');
    const warmth = field('processing.profiles[1].correction.warmth');
    assert.deepEqual(
      [warmth.type, warmth.min, warmth.max, warmth.step],
      ['range', '-100', '100', '1'],
    );
    await edit(warmth, 25);
    await edit(field('processing.profiles[1].correction.hue.cyan'), -10);
    assert.equal(
      warmth.parentElement.querySelector('output').textContent,
      '+25',
    );
    await settle();
    assert.deepEqual(processing(fixture).profiles[1].correction, {
      ...zero,
      warmth: 25,
      hue: { ...zero.hue, cyan: -10 },
    });

    // Deleted from its page; the list keeps its row, with Undo.
    await click(button('Colour profiles'));
    await openPage('Movie night');
    await click(button('Delete profile'));
    await settle();
    assert.equal(window.location.hash, '#/settings/profiles');
    assert.deepEqual(
      processing(fixture).profiles.map((profile) => profile.name),
      ['Game'],
    );
    assert.equal(valueOf('Movie night'), 'Deleted. Undo');
    await click(button('Undo'));
    await settle();
    assert.deepEqual(
      processing(fixture).profiles.map((profile) => profile.name),
      ['Movie night', 'Game'],
    );
    assert.equal(processing(fixture).active_profile, 'ps5');
    assert.equal(valueOf('Movie night'), 'Not in use');

    // Undo is offered for a while; then the row only says what happened.
    await openPage('Game');
    t.mock.timers.enable({ apis: ['setTimeout'] });
    await click(button('Delete profile'));
    assert.equal(processing(fixture).active_profile, undefined);
    await act(async () => t.mock.timers.tick(undoForMs));
    assert.equal(button('Undo'), undefined);
    assert.equal(valueOf('Game'), 'Deleted. ');
    t.mock.timers.reset();
    await settle();

    // Removing the last profile leaves the list out, as the server does.
    await openPage('Movie night');
    await click(button('Delete profile'));
    await settle();
    assert.equal('profiles' in processing(fixture), false);

    // New profiles come from the guided calibration.
    await openPage('New profile (guided)');
    assert.equal(window.location.hash, '#/calibrate');
  } finally {
    await portal.unmount();
  }
});

test('profile errors land on their control; the list is capped', async () => {
  const many = Array.from({ length: profileLimits.count }, (_, index) => ({
    id: `p${index}`,
    name: `Profile ${index}`,
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
          profiles: many,
          active_profile: 'p3',
        },
      },
    }),
  );
  const { fixture } = portal;
  try {
    await openPage('Colour profiles');
    assert.equal(
      valueOf('New profile (guided)'),
      'You have the most Prysm keeps (32). Delete one first.',
    );
    await openPage('New profile (guided)');
    assert.equal(window.location.hash, '#/settings/profiles', 'stays');

    await openPage('Profile 3');
    fixture.failNext({
      kind: 'invalid',
      field: 'processing.profiles[3].correction.tint',
      message: 'must be -100 to 100',
    });
    const tint = field('processing.profiles[3].correction.tint');
    await edit(tint, 40);
    await settle();
    assert.equal(errorOf(tint), 'Prysm can’t use this value.');
    assert.equal($('.save-status').textContent, 'Not saved');
    await settle();
    assert.equal(saves(fixture), 1, 'not retried until edited');
    await edit(tint, 30);
    await settle();
    assert.equal(saves(fixture), 2);
    assert.equal(processing(fixture).profiles[3].correction.tint, 30);
    assert.equal(errorOf(tint), undefined);

    await click(button('Delete profile'));
    await settle();
    assert.equal(processing(fixture).profiles.length, profileLimits.count - 1);
    assert.equal(
      valueOf('New profile (guided)'),
      'Test patterns on the TV, step by step',
    );
  } finally {
    await portal.unmount();
  }
});

test('new profile ids are UUIDs no other profile uses', () => {
  const id = newProfileId();
  assert.match(id, uuid);
  assert.notEqual(newProfileId([id]), id);
});
