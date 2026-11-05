import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ApplicationError } from '@prysm/application-client';
import { createApplicationRuntime } from '@prysm/application-state';
import { fakeApplicationPort } from '../testkit/application';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const initial = { revision: '0', capture: null, output: null };
function deferred() {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test('RTK distinguishes submission, acceptance and completion without changing observed lighting', async () => {
  const fixture = fakeApplicationPort(initial);
  const reply = deferred();
  let request;
  fixture.client.setTarget = (value) => {
    request = value;
    return reply.promise;
  };
  let result = { kind: 'pending' };
  fixture.client.operation = async () => result;
  const application = createApplicationRuntime(fixture.client);
  const release = application.connect();
  await tick();
  const pending = application.store.dispatch(
    application.commands.setTarget('processed'),
  );
  assert.equal(application.store.getState().lastCommand.phase, 'submitting');
  assert.equal(application.store.getState().connection.snapshot.target, 'off');
  release(); // Caller disappearance must not cancel a command already sent.
  reply.resolve({ operation: request.id });
  await pending;
  assert.equal(application.store.getState().lastCommand.phase, 'accepted');
  assert.equal(application.store.getState().lastCommand.result, null);
  assert.equal(application.store.getState().connection.snapshot, null);
  await application.store.dispatch(
    application.commands.checkOperation(request.id),
  );
  assert.equal(application.store.getState().lastCommand.result.kind, 'pending');
  result = { kind: 'failed', failure: { message: 'Device did not open' } };
  await application.store.dispatch(
    application.commands.checkOperation(request.id),
  );
  assert.equal(application.store.getState().lastCommand.result.kind, 'failed');
  assert(!fixture.calls.includes('target:off'));
});

test('RTK preserves unknown outcomes and queries the same operation without replaying the command', async () => {
  const fixture = fakeApplicationPort(initial);
  let sends = 0;
  let uncertain;
  fixture.client.setTarget = async ({ id }) => {
    sends++;
    uncertain = new ApplicationError({
      kind: 'outcome_unknown',
      operation: id,
    });
    throw uncertain;
  };
  fixture.client.operation = async (id) => {
    assert.deepEqual(id, uncertain.detail.operation);
    return { kind: 'succeeded' };
  };
  const application = createApplicationRuntime(fixture.client);
  const release = application.connect();
  await tick();
  await assert.rejects(
    application.store.dispatch(application.commands.setTarget('processed')),
    (error) => error === uncertain,
  );
  const command = application.store.getState().lastCommand;
  assert.equal(command.phase, 'outcome_unknown');
  assert.deepEqual(command.failure.operation, command.id);
  assert.deepEqual(
    JSON.parse(JSON.stringify(application.store.getState())),
    application.store.getState(),
  );
  await application.store.dispatch(
    application.commands.checkOperation(command.id),
  );
  assert.equal(
    application.store.getState().lastCommand.result.kind,
    'succeeded',
  );
  assert.equal(sends, 1);
  assert.equal(application.store.getState().connection.snapshot.target, 'off');
  release();
});

test('a delayed earlier command cannot overwrite newer command feedback', async () => {
  const fixture = fakeApplicationPort(initial);
  const first = deferred();
  let firstId;
  fixture.client.setTarget = ({ id, target }) => {
    if (target === 'processed') {
      firstId = id;
      return first.promise;
    }
    return Promise.resolve({ operation: id });
  };
  const application = createApplicationRuntime(fixture.client);
  const release = application.connect();
  await tick();
  const old = application.store.dispatch(
    application.commands.setTarget('processed'),
  );
  const latest = await application.store.dispatch(
    application.commands.setTarget('off'),
  );
  first.resolve({ operation: firstId });
  await old;
  assert.deepEqual(
    application.store.getState().lastCommand.id,
    latest.operation,
  );
  assert.equal(application.store.getState().lastCommand.phase, 'accepted');
  release();
});

test('each host has its own Redux state and transport, and calibration stays outside Redux', async () => {
  const one = fakeApplicationPort(initial, '1');
  const two = fakeApplicationPort(initial, '2');
  const first = createApplicationRuntime(one.client);
  const second = createApplicationRuntime(two.client);
  const releaseFirst = first.connect();
  const releaseSecond = second.connect();
  await tick();
  await first.store.dispatch(first.commands.setTarget('processed'));
  await tick();
  assert.equal(first.store.getState().connection.snapshot.target, 'processed');
  assert.equal(second.store.getState().connection.snapshot.target, 'off');
  assert.equal(second.store.getState().lastCommand, null);
  const before = first.store.getState();
  const presentation = await first.client.calibration.open({
    instance: '1',
    request: '99',
  });
  assert.equal(first.store.getState(), before);
  assert(!JSON.stringify(before).includes(presentation.session));
  assert(!JSON.stringify(before).includes(presentation.pairing));
  await first.client.calibration.close({
    id: { instance: '1', request: '100' },
    session: presentation.session,
  });
  releaseFirst();
  assert.equal(second.store.getState().connection.status, 'connected');
  releaseSecond();
});

test('sequence gaps and instance changes clear actionable state and close the stream', async () => {
  for (const cursor of [
    { instance: '1', sequence: '2' },
    { instance: '2', sequence: '1' },
  ]) {
    const fixture = fakeApplicationPort(initial);
    let closes = 0;
    fixture.client.observe = async () => ({
      initial: fixture.snapshot(),
      updates: (async function* () {
        yield { kind: 'snapshot', snapshot: { ...fixture.snapshot(), cursor } };
      })(),
      close: () => {
        closes++;
      },
    });
    const application = createApplicationRuntime(fixture.client);
    const release = application.connect();
    await tick();
    assert.equal(
      application.store.getState().connection.status,
      'resync_required',
    );
    assert.equal(
      application.selectors.selectCanCommand(application.store.getState()),
      false,
    );
    assert.equal(application.store.getState().connection.snapshot, null);
    assert.equal(closes, 1);
    await assert.rejects(
      application.store.dispatch(application.commands.setTarget('processed')),
      /not synchronized/,
    );
    assert(!fixture.calls.includes('target:processed'));
    release();
    assert.equal(closes, 1);
  }
});
