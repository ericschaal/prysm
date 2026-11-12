import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApplicationRuntime } from '@prysm/application-state';
import { ApplicationError } from '@prysm/application-client';
import { fakeApplicationPort } from '../testkit/application';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const capture = { kind: 'device', path: '/dev/video0' };
const output = { kind: 'wled', address: '192.0.2.1', leds: { top: 4 } };

async function connected(fixture) {
  const application = createApplicationRuntime(fixture.client);
  const release = application.connect();
  await tick();
  return { application, release, state: () => application.store.getState() };
}

test('commands resolve with the runtime result and observed state follows the stream', async () => {
  const fixture = fakeApplicationPort({ capture, output });
  const { application, release, state } = await connected(fixture);
  assert.equal(state().connection.status, 'connected');
  await application.store.dispatch(application.commands.setTarget('processed'));
  assert.deepEqual(state().lastCommand, {
    id: 1,
    kind: 'setTarget',
    phase: 'succeeded',
    failure: null,
  });
  await tick();
  assert.equal(state().connection.snapshot.session.state, 'running');

  const revision = await application.store.dispatch(
    application.commands.configureOutput(0, { ...output, address: '192.0.2.2' }),
  );
  assert.equal(revision, 1);
  await tick();
  const settings = state().connection.snapshot.settings;
  assert.equal(settings.output.address, '192.0.2.2');
  assert.deepEqual(settings.capture, capture);
  assert.equal(settings.processing.brightness_percent, 100);
  release();
});

test('stale revisions and server failures are recorded on the latest command', async () => {
  const fixture = fakeApplicationPort({ capture, output, revision: 3 });
  const { application, release, state } = await connected(fixture);
  await assert.rejects(
    application.store.dispatch(application.commands.configureCapture(2, capture)),
    (error) => error instanceof ApplicationError && error.failure.current === 3,
  );
  assert.equal(state().lastCommand.failure.kind, 'stale_revision');
  assert.deepEqual(fixture.calls, ['observe']);

  fixture.failNext({ kind: 'invalid', message: 'too many', field: 'output.leds' });
  await assert.rejects(
    application.store.dispatch(application.commands.configureOutput(3, output)),
  );
  assert.equal(state().lastCommand.failure.field, 'output.leds');
  fixture.failNext({ kind: 'faulted', message: 'runtime is faulted' });
  await assert.rejects(
    application.store.dispatch(application.commands.setTarget('off')),
  );
  assert.equal(state().lastCommand.kind, 'setTarget');
  assert.equal(state().lastCommand.phase, 'failed');
  release();
});

test('a delayed earlier command cannot overwrite newer command feedback', async () => {
  const fixture = fakeApplicationPort({ capture, output });
  let finish;
  fixture.client.setTarget = () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  const { application, release, state } = await connected(fixture);
  const slow = application.store.dispatch(application.commands.setTarget('off'));
  await application.store.dispatch(
    application.commands.configureOutput(0, output),
  );
  finish();
  await slow;
  assert.equal(state().lastCommand.kind, 'configureOutput');
  assert.equal(state().lastCommand.id, 2);
  release();
});

test('each runtime has its own store and the stream reconnects after loss', async () => {
  const fixture = fakeApplicationPort({ capture, output });
  const first = await connected(fixture);
  const second = await connected(fakeApplicationPort({ capture, output }));
  await first.application.store.dispatch(
    first.application.commands.setTarget('processed'),
  );
  await tick();
  assert.equal(first.state().connection.snapshot.target, 'processed');
  assert.equal(second.state().connection.snapshot.target, 'off');

  let fail = true;
  const observe = fixture.client.observe;
  fixture.client.observe = (signal) => {
    if (fail) {
      fail = false;
      return Promise.reject(new Error('connection refused'));
    }
    return observe(signal);
  };
  first.release();
  const release = first.application.connect();
  await tick();
  assert.equal(first.state().connection.status, 'disconnected');
  assert.equal(first.state().connection.error, 'connection refused');
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(first.state().connection.status, 'connected');
  release();
  second.release();
});

test('forbidden streams stop retrying', async () => {
  const fixture = fakeApplicationPort({});
  let attempts = 0;
  fixture.client.observe = () => {
    attempts++;
    return Promise.reject(
      new ApplicationError({ kind: 'forbidden', message: 'denied' }),
    );
  };
  const { release, state } = await connected(fixture);
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(attempts, 1);
  assert.equal(state().connection.error, 'denied');
  release();
});

test('shared observation closes once after the last consumer releases it', async () => {
  const fixture = fakeApplicationPort({});
  const application = createApplicationRuntime(fixture.client);
  const first = application.connect();
  const second = application.connect();
  await tick();
  first();
  first();
  assert.equal(application.store.getState().connection.status, 'connected');
  second();
  await tick();
  assert.equal(application.selectors.selectSnapshot(application.store.getState()), null);
  assert.deepEqual(fixture.calls, ['observe', 'unsubscribe']);
});

test('an observation acquired after release is closed without replacing newer state', async () => {
  const fixture = fakeApplicationPort({});
  let resolveLate;
  let calls = 0;
  let closed = 0;
  const observe = fixture.client.observe;
  fixture.client.observe = (signal) =>
    ++calls === 1
      ? new Promise((resolve) => {
          resolveLate = resolve;
        })
      : observe(signal);
  const application = createApplicationRuntime(fixture.client);
  application.connect()();
  const release = application.connect();
  await tick();
  resolveLate({
    initial: { ...fixture.snapshot(), target: 'device_native' },
    updates: (async function* () {})(),
    close() {
      closed++;
    },
  });
  await tick();
  assert.equal(closed, 1);
  assert.equal(
    application.selectors.selectSnapshot(application.store.getState()).target,
    'off',
  );
  release();
});
