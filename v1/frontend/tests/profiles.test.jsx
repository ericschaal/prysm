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
  $$,
  button,
  click,
  edit,
  errorOf,
  field,
  openPage,
  renderPortal,
  sleep,
  text,
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

test('colour profiles are listed, renamed, fine-tuned, used and deleted live, with Undo', async (t) => {
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
  const names = () => $$('.profile-name input').map((input) => input.value);
  const deletes = () => $$('button').filter((b) => b.textContent === 'Delete');
  try {
    await openPage('Colour profiles');
    assert.equal(window.location.hash, '#/settings/profiles');
    assert.deepEqual(names(), ['Movie night', 'PS5']);
    assert.deepEqual(
      $$('.profile-tag').map((tag) => tag.textContent),
      ['In use', ''],
    );
    assert.equal($('.save-status').textContent, 'All changes saved');

    // A name is saved whole, trimmed, on Enter (or leaving the field).
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

    // Fine-tuning a profile not in use offers to use it.
    await click(button('Use it now'));
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

    // Deleted at once; the row stays, with Undo, so nothing moves.
    const rows = $$('.profile-row').length;
    await click(deletes()[0]);
    await settle();
    assert.deepEqual(
      processing(fixture).profiles.map((profile) => profile.name),
      ['Game'],
    );
    assert.equal($$('.profile-row').length, rows);
    assert.equal(
      $('.profile-row.deleted').textContent,
      'Deleted “Movie night”. Undo',
    );
    await click(button('Undo'));
    await settle();
    assert.deepEqual(
      processing(fixture).profiles.map((profile) => profile.name),
      ['Movie night', 'Game'],
    );
    assert.equal(processing(fixture).active_profile, 'ps5');

    // Undo is offered for a while; then the row only says what happened.
    t.mock.timers.enable({ apis: ['setTimeout'] });
    await click(deletes()[1]);
    assert.equal(processing(fixture).active_profile, undefined);
    await act(async () => t.mock.timers.tick(undoForMs));
    assert.equal(button('Undo'), undefined);
    assert.equal($('.profile-row.deleted').textContent, 'Deleted “Game”.');
    t.mock.timers.reset();
    await settle();

    // Removing the last profile leaves the list out, as the server does.
    await click(deletes()[0]);
    await settle();
    assert.equal('profiles' in processing(fixture), false);

    // New profiles come from the guided calibration.
    await click(button('New profile (guided)'));
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
    assert.equal(button('New profile (guided)').disabled, true);
    assert(text().includes('You have the most profiles Prysm keeps (32).'));

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

    await click($$('button').find((b) => b.textContent === 'Delete'));
    await settle();
    assert.equal(processing(fixture).profiles.length, profileLimits.count - 1);
    assert.equal(button('New profile (guided)').disabled, false);
  } finally {
    await portal.unmount();
  }
});

test('new profile ids are UUIDs no other profile uses', () => {
  const id = newProfileId();
  assert.match(id, uuid);
  assert.notEqual(newProfileId([id]), id);
});
