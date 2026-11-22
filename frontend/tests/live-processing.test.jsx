/**
 * `useLiveProcessing` on its own, over the fixture's state stream: the save
 * queue and the rebase of a stale draft.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { fakeApplicationPort } from '../testkit/application';
import {
  processingDelayMs,
  rebased,
  useLiveProcessing,
} from '@prysm/web-portal';
import { mountDom, sleep } from './dom';

/** Renders the hook over `fixture`; `live()` is its latest result. */
async function mountLive(fixture) {
  const dom = mountDom();
  let latest;
  function Probe() {
    const [snapshot, setSnapshot] = useState(fixture.snapshot);
    useEffect(() => {
      const controller = new AbortController();
      void fixture.client.observe(controller.signal).then(async (stream) => {
        for await (const next of stream.updates) setSnapshot(next);
      });
      return () => controller.abort();
    }, []);
    const live = useLiveProcessing({
      processing: snapshot.settings.processing,
      revision: snapshot.revision,
      disabled: false,
      save: (expected, processing) =>
        fixture.client.replaceSettings(expected, {
          ...fixture.snapshot().settings,
          processing,
        }),
    });
    useEffect(() => {
      latest = live;
    });
    return null;
  }
  const root = createRoot(document.getElementById('root'));
  await act(async () => root.render(<Probe />));
  return {
    live: () => latest,
    async unmount() {
      await act(async () => root.unmount());
      dom.window.close();
    },
  };
}

const brightness = (fixture) =>
  fixture.snapshot().settings.processing.brightness_percent;

test('a commit asked for while a save is on its way resolves once its own value is saved', async () => {
  // It shared the save on its way: it resolved with the earlier value
  // saved, and its own followed later, unawaited.
  const fixture = fakeApplicationPort({});
  const probe = await mountLive(fixture);
  try {
    const release = fixture.holdSaves();
    let first;
    let second;
    await act(async () => {
      first = probe.live().commit({
        ...probe.live().value,
        brightness_percent: 40,
      });
    });
    await act(async () => {
      second = probe
        .live()
        .commit({ ...probe.live().value, brightness_percent: 60 })
        .then((saved) => [saved?.brightness_percent, brightness(fixture)]);
    });
    await act(async () => {
      release();
      assert.equal((await first)?.brightness_percent, 40);
      // Read the moment it resolves: nothing may land after it.
      assert.deepEqual(await second, [60, 60]);
    });
    assert.equal(brightness(fixture), 60);
    assert.equal(
      fixture.calls.filter((call) => call === 'replaceSettings').length,
      2,
      'each value once, in order',
    );
  } finally {
    await probe.unmount();
  }
});

test('a queued save goes against the revision the save before it returned', async () => {
  const fixture = fakeApplicationPort({ revision: 7 });
  const probe = await mountLive(fixture);
  try {
    let results;
    // Both asked for before any render shows the first one's revision.
    await act(async () => {
      const value = probe.live().value;
      results = await Promise.all([
        probe.live().commit({ ...value, brightness_percent: 30 }),
        probe.live().commit({ ...value, brightness_percent: 20 }),
      ]);
    });
    assert.deepEqual(
      results.map((saved) => saved?.brightness_percent),
      [30, 20],
    );
    assert.equal(brightness(fixture), 20);
    assert.equal(fixture.snapshot().revision, 9);
  } finally {
    await probe.unmount();
  }
});

const calibration = (id, name, warmth = 0) => ({
  id,
  name,
  correction: {
    warmth,
    tint: 0,
    midtones: 0,
    red: { hue: 0, strength: 0 },
    green: { hue: 0, strength: 0 },
    blue: { hue: 0, strength: 0 },
  },
});
const controls = (calibrations, active) => ({
  brightness_percent: 100,
  style: { kind: 'natural' },
  sample_density: 1,
  remove_black_bars: false,
  ...(calibrations.length ? { calibrations } : {}),
  ...(active === undefined ? {} : { active_calibration: active }),
});

test('a stale draft is rebased one calibration at a time', () => {
  // The whole list was one control: the draft's list won, and a rename made
  // on another phone was undone.
  const movie = calibration('movie', 'Movie');
  const game = calibration('game', 'Game');
  const base = controls([movie, game], 'movie');
  // This phone: Movie adjusted, a new draft added and made active.
  const mine = controls(
    [calibration('movie', 'Movie', 12), game, calibration('new', 'Calibrated')],
    'new',
  );
  // Another phone: Game renamed, and a third one added.
  const theirs = controls(
    [movie, calibration('game', 'Games'), calibration('ps5', 'PS5')],
    'movie',
  );
  assert.deepEqual(
    rebased(theirs, base, mine),
    controls(
      [
        calibration('movie', 'Movie', 12),
        calibration('game', 'Games'),
        calibration('ps5', 'PS5'),
        calibration('new', 'Calibrated'),
      ],
      'new',
    ),
  );
  // Deleted on either side is gone; the other's change to it does not bring it back.
  assert.deepEqual(
    rebased(
      controls([movie], 'movie'),
      base,
      controls([movie, calibration('game', 'Games')], 'movie'),
    ),
    controls([movie], 'movie'),
  );
  assert.deepEqual(
    rebased(
      controls([movie, calibration('game', 'Games')], 'movie'),
      base,
      controls([game], undefined),
    ),
    controls([calibration('game', 'Games')], undefined),
  );
  // Nothing left: absent, as the server sends it.
  assert.deepEqual(
    rebased(controls([movie], 'movie'), base, controls([], undefined)),
    controls([], undefined),
  );
});

test('a save after a failed one answers its error, so later changes save again', async () => {
  // The failed control kept "Not saved." and every later change waited
  // for it to be edited.
  const fixture = fakeApplicationPort({});
  const probe = await mountLive(fixture);
  try {
    fixture.failNext({ kind: 'unavailable', message: 'no answer' });
    await act(async () => {
      await probe
        .live()
        .commit(
          { ...probe.live().value, brightness_percent: 40 },
          'processing.brightness_percent',
        );
    });
    assert.equal(probe.live().state, 'failed');
    // Another control is saved at once (as Delete on Calibrations is).
    await act(async () => {
      await probe
        .live()
        .commit(
          { ...probe.live().value, remove_black_bars: true },
          'processing.remove_black_bars',
        );
    });
    assert.notEqual(probe.live().state, 'failed');
    assert.deepEqual(probe.live().scope.errors, {});
    // A change after it saves by itself.
    await act(async () =>
      probe.live().replace({ ...probe.live().value, sample_density: 64 }),
    );
    await act(async () => sleep(processingDelayMs + 100));
    assert.equal(
      fixture.snapshot().settings.processing.sample_density,
      64,
      'saved after the usual wait',
    );
  } finally {
    await probe.unmount();
  }
});
