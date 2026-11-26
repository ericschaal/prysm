import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApplicationRuntime } from '@prysm/application-state';
import { ApplicationError } from '@prysm/application-client';
import { httpApplicationClient } from '@prysm/application-client/http';
import { fakeApplicationPort } from '../testkit/application';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const capture = { kind: 'device', device: '/dev/video0' };
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
  await application.store.dispatch(
    application.commands.setTarget('screen_sync'),
  );
  await tick();
  assert.equal(state().connection.snapshot.session.state, 'screen_sync');

  const revision = await application.store.dispatch(
    application.commands.configureOutput(0, {
      ...output,
      address: '192.0.2.2',
    }),
  );
  assert.equal(revision, 1);
  await tick();
  const settings = state().connection.snapshot.settings;
  assert.equal(settings.output.address, '192.0.2.2');
  assert.deepEqual(settings.capture, capture);
  assert.equal(settings.processing.brightness_percent, 100);
  release();
});

test('stale revisions and server failures reject the command', async () => {
  const fixture = fakeApplicationPort({ capture, output, revision: 3 });
  const { application, release } = await connected(fixture);
  await assert.rejects(
    application.store.dispatch(
      application.commands.configureCapture(2, capture),
    ),
    (error) =>
      error instanceof ApplicationError &&
      error.failure.kind === 'stale_revision' &&
      error.failure.current === 3,
  );
  assert.deepEqual(fixture.calls, ['observe']);

  fixture.failNext({
    kind: 'invalid',
    message: 'too many',
    field: 'output.leds',
  });
  await assert.rejects(
    application.store.dispatch(application.commands.configureOutput(3, output)),
    (error) => error.failure.field === 'output.leds',
  );
  fixture.failNext({ kind: 'faulted', message: 'runtime is faulted' });
  await assert.rejects(
    application.store.dispatch(application.commands.setTarget('off')),
    (error) => error.failure.kind === 'faulted',
  );
  release();
});

test('each runtime has its own store and the stream reconnects after loss', async () => {
  const fixture = fakeApplicationPort({ capture, output });
  const first = await connected(fixture);
  const second = await connected(fakeApplicationPort({ capture, output }));
  await first.application.store.dispatch(
    first.application.commands.setTarget('screen_sync'),
  );
  await tick();
  assert.equal(first.state().connection.snapshot.target, 'screen_sync');
  assert.equal(second.state().connection.snapshot.target, 'off');

  let fail = true;
  const observe = fixture.client.observe;
  fixture.client.observe = (signal) => {
    if (fail) {
      fail = false;
      return Promise.reject(
        new ApplicationError({
          kind: 'unavailable',
          message: 'connection refused',
        }),
      );
    }
    return observe(signal);
  };
  first.release();
  const release = first.application.connect();
  await tick();
  assert.equal(first.state().connection.status, 'disconnected');
  assert.equal(first.state().connection.error.message, 'connection refused');
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(first.state().connection.status, 'connected');
  release();
  second.release();
});

test('a lost stream keeps its last state but saves nothing into it', async () => {
  const fixture = fakeApplicationPort({ capture, output });
  const { application, release, state } = await connected(fixture);
  fixture.drop();
  await tick();
  assert.equal(state().connection.status, 'disconnected');
  assert.deepEqual(state().connection.snapshot.settings.capture, capture);
  await assert.rejects(
    application.store.dispatch(
      application.commands.configureCapture(0, capture),
    ),
    (error) =>
      error instanceof ApplicationError &&
      error.failure.kind === 'stale_revision',
  );
  assert.deepEqual(fixture.calls, ['observe', 'unsubscribe']);
  release();
});

test('a state the page came with is current until the stream replaces it, kept when the stream fails, gone on release', async () => {
  const fixture = fakeApplicationPort({ capture, output });
  const page = { ...fixture.snapshot(), target: 'screen_sync' };
  const application = createApplicationRuntime(fixture.client, page);
  const state = () => application.store.getState().connection;
  assert.equal(state().status, 'connected');
  assert.equal(state().snapshot, page);
  // Asking for the stream keeps it, current, until the stream's first.
  const observe = fixture.client.observe;
  let fail = true;
  fixture.client.observe = (signal) => {
    if (fail) {
      fail = false;
      return Promise.reject(
        new ApplicationError({
          kind: 'unavailable',
          message: 'connection refused',
        }),
      );
    }
    return observe(signal);
  };
  const release = application.connect();
  assert.equal(state().status, 'connected');
  assert.equal(state().snapshot, page);
  await tick();
  assert.equal(state().status, 'disconnected');
  assert.equal(state().snapshot, page, 'kept while it reconnects');
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(state().status, 'connected');
  assert.equal(state().snapshot.target, 'off', 'the stream’s own');
  release();
  await tick();
  assert.equal(state().snapshot, null);
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
  assert.deepEqual(state().connection.error, {
    kind: 'forbidden',
    message: 'denied',
  });
  release();
});

test('a stream event this page can’t read stops the stream, not retried', async () => {
  let attempts = 0;
  const client = httpApplicationClient({
    fetch: async () => {
      attempts++;
      return new Response('data: {"kind":"snapshot"}\n\n', {
        headers: { 'content-type': 'text/event-stream' },
      });
    },
  });
  const application = createApplicationRuntime(client);
  const release = application.connect();
  await new Promise((resolve) => setTimeout(resolve, 300));
  const { connection } = application.store.getState();
  assert.equal(attempts, 1);
  assert.equal(connection.status, 'disconnected');
  assert.equal(connection.error.kind, 'internal');
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
  assert.equal(application.store.getState().connection.snapshot, null);
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
    initial: { ...fixture.snapshot(), target: 'effects' },
    updates: (async function* () {})(),
    close() {
      closed++;
    },
  });
  await tick();
  assert.equal(closed, 1);
  assert.equal(application.store.getState().connection.snapshot.target, 'off');
  release();
});
