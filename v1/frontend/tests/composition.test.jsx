import { createApplicationRuntime } from '@prysm/application-state';
import './lifecycle.test.jsx';
import './state.test.jsx';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { ServerWebPortal } from '../hosts/server/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { fakeApplication, fakeApplicationPort } from '../testkit/application';

const resolution = [320, 180];
const output = { address: '192.0.2.1', leds: [10, 20, 30, 40] };

test('both hosts mount their panels, submit typed settings, and receive snapshots', async () => {
  for (const [Host, capture, heading, field, value, savedField] of [
    [
      ServerWebPortal,
      {
        adapter: 'device',
        settings: {
          source: 'device',
          path: '/dev/video0',
          resolution,
          rate: 30,
        },
      },
      'Video device capture',
      'location',
      '/dev/video1',
      'path',
    ],
    [
      ServerWebPortal,
      {
        adapter: 'device',
        settings: { source: 'file', path: 'old.mp4', resolution, rate: 30 },
      },
      'Video device capture',
      'location',
      'new.mp4',
      'path',
    ],
    [
      ServerWebPortal,
      { adapter: 'remote', settings: { peer: '127.0.0.1:9000' } },
      'Remote capture',
      'peer',
      '127.0.0.1:9001',
      'peer',
    ],
    [
      WebosWebPortal,
      { resolution, rate: 30 },
      'webOS capture',
      'rate',
      '60',
      'rate',
    ],
  ]) {
    const dom = new JSDOM('<div id="root"></div>');
    Object.assign(globalThis, {
      window: dom.window,
      document: dom.window.document,
      FormData: dom.window.FormData,
      IS_REACT_ACT_ENVIRONMENT: true,
    });
    const client = fakeApplication({ revision: '0', capture, output });
    const root = createRoot(document.getElementById('root'));
    try {
      await act(async () => root.render(<Host application={client} />));
      assert(document.body.textContent.includes(heading));
      assert(document.body.textContent.includes('WLED output'));
      assert(
        !document.body.textContent.includes(
          Host === ServerWebPortal ? 'webOS capture' : 'Video device capture',
        ),
      );

      document.querySelector(`[name="${field}"]`).value = value;
      await act(async () =>
        document
          .querySelector('form')
          .dispatchEvent(
            new dom.window.Event('submit', { bubbles: true, cancelable: true }),
          ),
      );
      assert.equal(
        (capture.adapter
          ? client.selectors.selectSnapshot(client.store.getState()).capture
              .settings
          : client.selectors.selectSnapshot(client.store.getState()).capture)[
          savedField
        ],
        field === 'rate' ? Number(value) : value,
      );
      assert.equal(
        client.selectors.selectSnapshot(client.store.getState()).revision,
        '1',
      );
      assert.deepEqual(
        client.selectors.selectSnapshot(client.store.getState()).output,
        output,
      );
      assert(document.body.textContent.includes('Settings revision: 1'));
      assert(
        document
          .querySelector('[role="status"]')
          .textContent.includes('Accepted operation '),
      );

      document.querySelector('[name="address"]').value = '192.0.2.2';
      await act(async () =>
        document
          .querySelectorAll('form')[1]
          .dispatchEvent(
            new dom.window.Event('submit', { bubbles: true, cancelable: true }),
          ),
      );
      assert.equal(
        client.selectors.selectSnapshot(client.store.getState()).output.address,
        '192.0.2.2',
      );
      assert.deepEqual(
        client.selectors.selectSnapshot(client.store.getState()).output.leds,
        output.leds,
      );
      assert.equal(
        client.selectors.selectSnapshot(client.store.getState()).revision,
        '2',
      );

      client.client.configureOutput = async () => {
        throw new Error('Fixture endpoint unavailable.');
      };
      await act(async () =>
        document
          .querySelectorAll('form')[1]
          .dispatchEvent(
            new dom.window.Event('submit', { bubbles: true, cancelable: true }),
          ),
      );
      assert.equal(
        document.querySelector('[role="alert"]').textContent,
        'Fixture endpoint unavailable.',
      );
      assert.equal(
        client.selectors.selectSnapshot(client.store.getState()).revision,
        '2',
      );
    } finally {
      await act(async () => root.unmount());
      dom.window.close();
    }
  }
});

test('the fixture rejects stale revisions and releases subscriptions', async () => {
  const fixture = fakeApplicationPort({
    revision: '0',
    capture: { resolution, rate: 30 },
    output,
  });
  const abort = new AbortController();
  const observation = await fixture.client.observe(abort.signal);
  await fixture.client.configureCapture({
    id: { instance: '1', request: '1' },
    expected: '0',
    settings: { resolution, rate: 60 },
  });
  await assert.rejects(
    fixture.client.configureOutput({
      id: { instance: '1', request: '2' },
      expected: '0',
      settings: output,
    }),
    /conflict/,
  );
  assert.equal(fixture.snapshot().revision, '1');
  observation.close();
  assert.equal(
    fixture.calls.filter((call) => call === 'unsubscribe').length,
    1,
  );
});

// TV tests use the same real shared UI and injected clients as the standalone bundles.
import { WebosApp } from '../hosts/webos/tv-ui/app';
import { WebosSlimApp } from '../hosts/webos-slim/tv-ui/app';
import { fakeTvHost, fakePatternPresenter } from '../testkit/tv';
import { ApplicationError } from '@prysm/application-client';
import { unavailableApplicationBridge } from '@prysm/application-client/adapters';
import { createPresentationController } from '../packages/tv-ui/presentation';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test('TV assemblies route commands correctly and UI teardown leaves normal lighting running', async () => {
  for (const embedded of [true, false]) {
    const dom = new JSDOM('<div id="root"></div>');
    Object.assign(globalThis, {
      window: dom.window,
      document: dom.window.document,
      FormData: dom.window.FormData,
      IS_REACT_ACT_ENVIRONMENT: true,
    });
    const application = fakeApplicationPort(
      { revision: '0', capture: { resolution, rate: 30 }, output },
      embedded ? '1' : '2',
    );
    const host = fakeTvHost(!embedded);
    const patterns = fakePatternPresenter();
    const cleanupErrors = [];
    let exits = 0;
    const props = {
      platform: host.client,
      presenter: patterns.presenter,
      onExit: () => {
        exits++;
      },
      onCleanupFailure: (error) => cleanupErrors.push(error),
    };
    const root = createRoot(document.getElementById('root'));
    let unmounted = false;
    try {
      await act(async () =>
        root.render(
          embedded ? (
            <WebosApp
              localApplication={application.client}
              portalUrl="http://tv.local:8787"
              {...props}
            />
          ) : (
            <WebosSlimApp remoteApplication={application.client} {...props} />
          ),
        ),
      );
      assert(document.body.textContent.includes('Prysm on TV'));
      assert.equal(
        !!document.querySelector('a[href="http://tv.local:8787"]'),
        embedded,
      );
      const button = (text) =>
        [...document.querySelectorAll('button')].find(
          (item) => item.textContent === text,
        );
      await act(async () => button('Inspect TV service').click());
      assert.deepEqual(host.calls, ['inspect_service']);
      assert(!application.calls.includes('inspect_service'));
      await act(async () => button('Turn lighting on').click());
      assert.equal(application.snapshot().target, 'processed');
      assert.equal(
        application.snapshot().cursor.instance,
        embedded ? '1' : '2',
      );
      button('Turn lighting on').focus();
      await act(async () =>
        document.activeElement.dispatchEvent(
          new dom.window.KeyboardEvent('keydown', {
            key: 'ArrowRight',
            bubbles: true,
          }),
        ),
      );
      assert.equal(document.activeElement, button('Turn lighting off'));
      await act(async () => button('Start calibration').click());
      assert(document.body.textContent.includes('fixture-private-pairing'));
      assert(
        !JSON.stringify(application.snapshot()).includes('fixture-private'),
      );
      await act(async () =>
        document.querySelector('main').dispatchEvent(
          new dom.window.KeyboardEvent('keydown', {
            key: 'Escape',
            bubbles: true,
          }),
        ),
      );
      assert(application.calls.includes('calibration:released'));
      assert(application.calls.includes('calibration:close'));
      assert.equal(exits, 0);
      await act(async () => button('Start calibration').click());
      await act(async () => root.unmount());
      unmounted = true;
      assert(application.calls.includes('unsubscribe'));
      assert.equal(application.snapshot().target, 'processed');
      assert(!application.calls.includes('target:off'));
      assert(patterns.calls.includes('release'));
      assert.deepEqual(cleanupErrors, []);
    } finally {
      if (!unmounted) await act(async () => root.unmount());
      dom.window.close();
    }
  }
});

test('uncertain outcomes identify the operation and production stubs cannot claim success', async () => {
  const operation = { instance: '1', request: '99' };
  const unknown = new ApplicationError({ kind: 'outcome_unknown', operation });
  assert(
    unknown.message.includes('1/99'),
    'visible errors must retain the operation to query',
  );
  await assert.rejects(
    unavailableApplicationBridge('Luna').setTarget({
      id: operation,
      target: 'processed',
    }),
    (error) => error.detail.kind === 'unavailable',
  );
});

test('store clears stale state on resync and releases observers without commanding the pipeline', async () => {
  const fixture = fakeApplicationPort({ revision: '0', capture: {}, output });
  const store = createApplicationRuntime(fixture.client);
  const release = store.connect();
  await tick();
  assert.equal(store.store.getState().connection.status, 'connected');
  const accepted = await store.store.dispatch(
    store.commands.setTarget('processed'),
  );
  assert.equal(
    (await fixture.client.operation(accepted.operation)).kind,
    'succeeded',
  );
  fixture.resync();
  await tick();
  assert.equal(store.store.getState().connection.status, 'resync_required');
  assert.equal(store.selectors.selectSnapshot(store.store.getState()), null);
  await assert.rejects(
    store.store.dispatch(store.commands.setTarget('off')),
    /not synchronized/,
  );
  release();
  assert(fixture.calls.includes('unsubscribe'));
  assert.equal(fixture.snapshot().target, 'processed');
});

test('late calibration open and playback failures clean up only their private presentation', async () => {
  const fixture = fakeApplicationPort({ revision: '0', capture: {}, output });
  const patterns = fakePatternPresenter();
  const operation = { instance: '1', request: '7' };
  let resolveOpen;
  fixture.client.calibration.open = () =>
    new Promise((resolve) => {
      resolveOpen = resolve;
    });
  const controller = createPresentationController(
    fixture.client.calibration,
    patterns.presenter,
  );
  const pending = controller.start(operation);
  await controller.stop();
  resolveOpen({ session: 'late', pairing: 'private', pattern: { id: 'gray' } });
  assert.equal(await pending, null);
  assert(!patterns.calls.some((call) => call.startsWith('present:')));
  assert(fixture.calls.includes('calibration:close'));

  const other = fakeApplicationPort({ revision: '0', capture: {}, output });
  const failed = createPresentationController(other.client.calibration, {
    present: async () => {
      throw new Error('Playback failed');
    },
    release: () => {},
  });
  await assert.rejects(failed.start(operation), /Playback failed/);
  assert(other.calls.includes('calibration:failed'));
  assert(other.calls.includes('calibration:close'));
  assert(!other.calls.includes('target:off'));
});

test('embedded TV and browser portal share one application; either UI may disconnect independently', async () => {
  const dom = new JSDOM('<div id="tv"></div><div id="portal"></div>');
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    FormData: dom.window.FormData,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const fixture = fakeApplicationPort({
    revision: '0',
    capture: { resolution, rate: 30 },
    output,
  });
  const host = fakeTvHost(false);
  const patterns = fakePatternPresenter();
  const browser = createApplicationRuntime(fixture.client);
  const tvRoot = createRoot(document.getElementById('tv'));
  const browserRoot = createRoot(document.getElementById('portal'));
  let tvClosed = false;
  try {
    await act(async () => {
      tvRoot.render(
        <WebosApp
          localApplication={fixture.client}
          platform={host.client}
          presenter={patterns.presenter}
          portalUrl="http://tv.local:8787"
          onExit={() => {}}
          onCleanupFailure={() => {}}
        />,
      );
      browserRoot.render(<WebosWebPortal application={browser} />);
    });
    const light = [...document.querySelectorAll('#tv button')].find(
      (button) => button.textContent === 'Turn lighting on',
    );
    await act(async () => light.click());
    assert.equal(
      browser.selectors.selectSnapshot(browser.store.getState()).target,
      'processed',
    );
    await act(async () => tvRoot.unmount());
    tvClosed = true;
    await act(async () =>
      browser.store.dispatch(
        browser.commands.configureOutput('0', {
          ...output,
          address: '192.0.2.44',
        }),
      ),
    );
    assert.equal(fixture.snapshot().output.address, '192.0.2.44');
    assert.equal(fixture.snapshot().target, 'processed');
    assert(!fixture.calls.includes('target:off'));
  } finally {
    if (!tvClosed) await act(async () => tvRoot.unmount());
    await act(async () => browserRoot.unmount());
    dom.window.close();
  }
});

test('server selects adapters without leaking local capture settings into remote transport', async () => {
  const dom = new JSDOM('<div id="root"></div>');
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    FormData: dom.window.FormData,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const client = fakeApplication({
    revision: '0',
    output,
    capture: {
      adapter: 'device',
      settings: { source: 'file', path: 'fixture.mp4', resolution, rate: 24 },
    },
  });
  const root = createRoot(document.getElementById('root'));
  try {
    await act(async () =>
      root.render(<ServerWebPortal application={client} />),
    );
    assert.deepEqual(
      [...document.querySelector('[name="source"]').options].map(
        (option) => option.value,
      ),
      ['device', 'file'],
    );
    for (const adapter of ['remote', 'device']) {
      const previous = client.selectors.selectSnapshot(
        client.store.getState(),
      ).capture;
      await act(async () => {
        const select = document.querySelector('[name="adapter"]');
        select.value = adapter;
        select.dispatchEvent(new dom.window.Event('change', { bubbles: true }));
      });
      assert.deepEqual(
        client.selectors.selectSnapshot(client.store.getState()).capture,
        previous,
        'choosing a panel must not issue a command',
      );
      const field = adapter === 'remote' ? 'peer' : 'location';
      document.querySelector(`[name="${field}"]`).value =
        adapter === 'remote' ? '127.0.0.1:9000' : '/dev/video1';
      await act(async () =>
        document
          .querySelector('form')
          .dispatchEvent(
            new dom.window.Event('submit', { bubbles: true, cancelable: true }),
          ),
      );
      const saved = client.selectors.selectSnapshot(
        client.store.getState(),
      ).capture;
      assert.equal(saved.adapter, adapter);
      if (adapter === 'remote')
        assert.deepEqual(saved.settings, { peer: '127.0.0.1:9000' });
      else {
        assert.equal(saved.settings.source, 'device');
        assert.equal(saved.settings.path, '/dev/video1');
        assert(!('peer' in saved.settings));
      }
    }
    assert.equal(
      client.selectors.selectSnapshot(client.store.getState()).revision,
      '2',
    );
    assert.deepEqual(
      client.selectors.selectSnapshot(client.store.getState()).output,
      output,
    );
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
  }
});

test('rerendering the TV cleanup callback does not end an active presentation', async () => {
  const dom = new JSDOM('<div id="root"></div>');
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    FormData: dom.window.FormData,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const fixture = fakeApplicationPort({
    revision: '0',
    capture: { resolution, rate: 30 },
    output,
  });
  const host = fakeTvHost(false);
  const patterns = fakePatternPresenter();
  const root = createRoot(document.getElementById('root'));
  const render = () =>
    root.render(
      <WebosApp
        localApplication={fixture.client}
        platform={host.client}
        presenter={patterns.presenter}
        portalUrl="http://tv.local:8787"
        onExit={() => {}}
        onCleanupFailure={() => {}}
      />,
    );
  try {
    await act(async () => render());
    const start = [...document.querySelectorAll('button')].find(
      (button) => button.textContent === 'Start calibration',
    );
    await act(async () => start.click());
    assert(document.body.textContent.includes('fixture-private-pairing'));
    await act(async () => render());
    assert(!fixture.calls.includes('calibration:close'));
    await act(async () =>
      document.querySelector('main').dispatchEvent(
        new dom.window.KeyboardEvent('keydown', {
          key: 'Escape',
          bubbles: true,
        }),
      ),
    );
    assert(fixture.calls.includes('calibration:close'));
    assert(!document.body.textContent.includes('fixture-private-pairing'));
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
  }
});
