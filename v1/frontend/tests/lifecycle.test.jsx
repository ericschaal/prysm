import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApplicationRuntime } from '@prysm/application-state';
import { createPresentationController } from '../packages/tv-ui/presentation';
import { fakeApplicationPort } from '../testkit/application';
import { fakePatternPresenter } from '../testkit/tv';

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const operation = { instance: '1', request: '7' };
const initial = { revision: '0', capture: null, output: null };

test('overlapping stops await cleanup and failed cleanup can be retried before reopening', async () => {
  const fixture = fakeApplicationPort(initial);
  const close = deferred();
  const requests = [];
  fixture.client.calibration.close = (request) => {
    requests.push(request);
    return requests.length === 1 ? close.promise : Promise.resolve();
  };
  const controller = createPresentationController(
    fixture.client.calibration,
    fakePatternPresenter().presenter,
  );
  await controller.start(operation);
  const first = controller.stop();
  let settled = false;
  const second = controller.stop().finally(() => {
    settled = true;
  });
  const failures = Promise.all([
    assert.rejects(first, /connection lost/),
    assert.rejects(second, /connection lost/),
  ]);
  await tick();
  assert.equal(settled, false);
  assert.equal(requests.length, 1);
  await assert.rejects(controller.start(operation), /already/);
  close.reject(new Error('connection lost'));
  await failures;
  await assert.rejects(controller.start(operation), /already/);
  await controller.stop();
  assert.equal(requests.length, 2);
  assert.deepEqual(
    requests[1],
    requests[0],
    'retry keeps the close operation identity',
  );
  assert(await controller.start(operation));
  await controller.stop();
});

test('failed cleanup of a late open remains owned without starting playback', async () => {
  const fixture = fakeApplicationPort(initial);
  const open = deferred();
  const patterns = fakePatternPresenter();
  let closes = 0;
  fixture.client.calibration.open = () => open.promise;
  fixture.client.calibration.close = async () => {
    if (++closes === 1) throw new Error('connection lost');
  };
  const controller = createPresentationController(
    fixture.client.calibration,
    patterns.presenter,
  );
  const start = controller.start(operation);
  await controller.stop();
  open.resolve({ session: 'late', pairing: 'private', pattern: { id: 'gray' } });
  await assert.rejects(start, /connection lost/);
  await assert.rejects(controller.start(operation), /already/);
  await controller.stop();
  assert.equal(closes, 2);
  assert(!patterns.calls.some((call) => call.startsWith('present:')));
});

test('closing during readiness feedback cannot restore the calibration presentation', async () => {
  const fixture = fakeApplicationPort(initial);
  const ready = deferred();
  fixture.client.calibration.feedback = ({ playback }) =>
    playback.kind === 'ready' ? ready.promise : Promise.resolve();
  const controller = createPresentationController(
    fixture.client.calibration,
    fakePatternPresenter().presenter,
  );
  const start = controller.start(operation);
  await tick();
  await controller.stop();
  ready.resolve();
  assert.equal(await start, null);
  assert(fixture.calls.includes('calibration:close'));
});

test('playback release failure still closes the remote calibration session', async () => {
  const fixture = fakeApplicationPort(initial);
  const controller = createPresentationController(fixture.client.calibration, {
    present: async () => ({ kind: 'ready' }),
    release: () => {
      throw new Error('release failed');
    },
  });
  await controller.start(operation);
  await assert.rejects(controller.stop(), /release failed/);
  assert(fixture.calls.includes('calibration:close'));
  assert(
    !fixture.calls.includes('calibration:released'),
    'failed release must not claim cleanup success',
  );
});

test(
  'an overlapping calibration start cannot orphan the current session',
  { timeout: 1000 },
  async () => {
    const fixture = fakeApplicationPort(initial);
    const open = deferred();
    let opens = 0;
    fixture.client.calibration.open = () => {
      opens++;
      return open.promise;
    };
    const controller = createPresentationController(
      fixture.client.calibration,
      fakePatternPresenter().presenter,
    );
    const start = controller.start(operation);
    await assert.rejects(controller.start(operation), /already/);
    assert.equal(opens, 1);
    open.resolve({
      session: 'first',
      pairing: 'private',
      pattern: { id: 'gray' },
    });
    await start;
    await assert.rejects(controller.start(operation), /already/);
    await controller.stop();
  },
);

test('shared observation closes once after the last consumer releases it', async () => {
  const fixture = fakeApplicationPort(initial);
  const observe = fixture.client.observe;
  let closes = 0;
  fixture.client.observe = async (signal) => {
    const observation = await observe(signal);
    return {
      ...observation,
      close() {
        closes++;
        observation.close();
      },
    };
  };
  const store = createApplicationRuntime(fixture.client);
  const first = store.connect();
  const second = store.connect();
  await tick();
  first();
  first();
  assert.equal(closes, 0);
  assert.equal(store.store.getState().connection.status, 'connected');
  second();
  await tick();
  assert.equal(closes, 1);
  assert.equal(store.selectors.selectSnapshot(store.store.getState()), null);
  assert.deepEqual(fixture.calls, ['observe', 'unsubscribe']);
});

test('an observation acquired after disconnect is released without replacing a newer snapshot', async () => {
  const fixture = fakeApplicationPort(initial);
  const late = deferred();
  let calls = 0;
  let closed = 0;
  const observe = fixture.client.observe;
  fixture.client.observe = (signal) =>
    ++calls === 1 ? late.promise : observe(signal);
  const store = createApplicationRuntime(fixture.client);
  const release = store.connect();
  release();
  const releaseNew = store.connect();
  await tick();
  late.resolve({
    initial: {
      ...initial,
      cursor: { instance: 'old', sequence: '0' },
      target: 'off',
    },
    updates: (async function* () {})(),
    close() {
      closed++;
    },
  });
  await tick();
  assert.equal(closed, 1);
  assert.equal(
    store.selectors.selectSnapshot(store.store.getState()).cursor.instance,
    '1',
  );
  releaseNew();
});
