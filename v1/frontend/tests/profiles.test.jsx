import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import {
  newProfileId,
  processingDelayMs,
  profileLimits,
} from '@prysm/web-portal';
import {
  $,
  $$,
  button,
  click,
  edit,
  errorOf,
  field,
  openSettings,
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
const active = () => field('processing.active_profile');
const enter = (element) =>
  act(async () =>
    element.dispatchEvent(
      new element.ownerDocument.defaultView.KeyboardEvent('keydown', {
        key: 'Enter',
        bubbles: true,
      }),
    ),
  );

test('colour profiles are created, renamed, edited, duplicated, chosen and deleted live', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await openSettings();
    assert.deepEqual(
      $$('option', active()).map((option) => [
        option.value,
        option.textContent,
      ]),
      [['', 'None (no correction)']],
    );
    assert.equal(button('Duplicate'), undefined);

    await click(button('New profile'));
    await settle();
    assert.equal(saves(fixture), 1);
    const [created] = processing(fixture).profiles;
    assert.match(created.id, uuid);
    assert.deepEqual(created, {
      id: created.id,
      name: 'New profile',
      correction: zero,
    });
    assert.equal(processing(fixture).active_profile, created.id);
    assert.equal(active().value, created.id);

    // A name is saved whole, trimmed, on Enter (or leaving the field).
    const name = field('processing.profiles[0].name');
    await edit(name, '');
    assert.equal(errorOf(name), 'Enter a name.');
    await edit(name, '  PS5 ');
    await settle();
    assert.equal(saves(fixture), 1);
    await enter(name);
    assert.equal(name.value, 'PS5');
    await settle();
    assert.equal(processing(fixture).profiles[0].name, 'PS5');
    assert.equal($('option:checked', active()).textContent, 'PS5');

    const warmth = field('processing.profiles[0].correction.warmth');
    assert.deepEqual(
      [warmth.type, warmth.min, warmth.max, warmth.step],
      ['range', '-100', '100', '1'],
    );
    await edit(warmth, 25);
    await edit(field('processing.profiles[0].correction.hue.cyan'), -10);
    assert.equal(
      warmth.parentElement.querySelector('output').textContent,
      '+25',
    );
    await settle();
    assert.deepEqual(processing(fixture).profiles[0].correction, {
      ...zero,
      warmth: 25,
      hue: { ...zero.hue, cyan: -10 },
    });

    await click(button('Duplicate'));
    await settle();
    const [, copy] = processing(fixture).profiles;
    assert.notEqual(copy.id, created.id);
    assert.equal(copy.name, 'PS5 copy');
    assert.deepEqual(
      copy.correction,
      processing(fixture).profiles[0].correction,
    );
    assert.equal(processing(fixture).active_profile, copy.id);

    await edit(active(), '');
    await settle();
    assert.equal('active_profile' in processing(fixture), false);
    assert.equal(
      field('processing.profiles[0].name'),
      null,
      'no editor without a profile',
    );

    await edit(active(), created.id);
    await click(button('Delete…'));
    assert(text().includes('Delete “PS5”? This cannot be undone.'));
    await click(button('Keep'));
    assert.equal(button('Delete profile'), undefined);
    await click(button('Delete…'));
    await click(button('Delete profile'));
    await settle();
    assert.deepEqual(
      processing(fixture).profiles.map((profile) => profile.name),
      ['PS5 copy'],
    );
    assert.equal('active_profile' in processing(fixture), false);

    // Removing the last profile leaves the list out, as the server does.
    await edit(active(), copy.id);
    await click(button('Delete…'));
    await click(button('Delete profile'));
    await settle();
    assert.equal('profiles' in processing(fixture), false);
    assert.equal(processing(fixture).brightness_percent, 100);
  } finally {
    await portal.unmount();
  }
});

test('profile errors from the server land on their control; the list is capped', async () => {
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
    await openSettings();
    assert.equal(button('New profile').disabled, true);
    assert.equal(button('Duplicate').disabled, true);
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

    fixture.failNext({
      kind: 'invalid',
      field: 'processing.active_profile',
      message: 'no profile has id `p5`',
    });
    await edit(active(), 'p5');
    await settle();
    assert.equal(errorOf(active()), 'Prysm can’t use this value.');
    await edit(active(), 'p6');
    await settle();
    assert.equal(processing(fixture).active_profile, 'p6');

    await click(button('Delete…'));
    await click(button('Delete profile'));
    await settle();
    assert.equal(processing(fixture).profiles.length, profileLimits.count - 1);
    assert.equal(button('New profile').disabled, false);
  } finally {
    await portal.unmount();
  }
});

test('new profile ids are UUIDs no other profile uses', () => {
  const id = newProfileId();
  assert.match(id, uuid);
  assert.notEqual(newProfileId([id]), id);
});
