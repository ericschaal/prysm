import './state.test.jsx';
import './http.test.jsx';
import './portal.test.jsx';
import './preview.test.jsx';
import './profiles.test.jsx';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { createApplicationRuntime } from '@prysm/application-state';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { WebosApp } from '../hosts/webos/tv-ui/app';
import { WebosSlimApp } from '../hosts/webos-slim/tv-ui/app';
import { fakeApplicationPort } from '../testkit/application';
import { fakeTvHost } from '../testkit/tv';
import { button, mountDom } from './dom';

const output = { kind: 'wled', address: '192.0.2.1', leds: { top: 10, left: 4 } };

test('TV assemblies route lighting to the application and platform commands to the TV', async () => {
  for (const embedded of [true, false]) {
    const dom = mountDom();
    const fixture = fakeApplicationPort({ capture: { kind: 'webos' }, output });
    const host = fakeTvHost(!embedded);
    let exits = 0;
    const props = { platform: host.client, onExit: () => exits++ };
    const root = createRoot(document.getElementById('root'));
    try {
      await act(async () =>
        root.render(
          embedded ? (
            <WebosApp localApplication={fixture.client} portalUrl="http://tv.local:8787" {...props} />
          ) : (
            <WebosSlimApp remoteApplication={fixture.client} {...props} />
          ),
        ),
      );
      assert(document.body.textContent.includes('Prysm on TV'));
      assert.equal(!!document.querySelector('a[href="http://tv.local:8787"]'), embedded);
      assert(document.body.textContent.includes(embedded ? 'Local capture: available' : 'forwarding frames'));
      await act(async () => button('Inspect TV service').click());
      assert.deepEqual(host.calls, ['inspect_service']);
      await act(async () => button('Turn lighting on').click());
      assert.equal(fixture.snapshot().target, 'processed');

      button('Turn lighting on').focus();
      await act(async () =>
        document.activeElement.dispatchEvent(
          new dom.window.KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }),
        ),
      );
      assert.equal(document.activeElement, button('Turn lighting off'));
      await act(async () =>
        document.querySelector('main').dispatchEvent(
          new dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
        ),
      );
      assert.equal(exits, 1);

      fixture.failNext({ kind: 'adapter', message: 'fake output cannot open' });
      await act(async () => button('Turn lighting off').click());
      assert.equal(document.querySelector('[role="status"]').textContent, 'fake output cannot open');
      await act(async () => root.unmount());
      // Leaving the UI releases the stream and never turns lighting off.
      assert(fixture.calls.includes('unsubscribe'));
      assert.equal(fixture.snapshot().target, 'processed');
    } finally {
      dom.window.close();
    }
  }
});

test('embedded TV and browser portal share one application; either may close', async () => {
  const dom = mountDom('<div id="tv"></div><div id="portal"></div>');
  const fixture = fakeApplicationPort({ capture: { kind: 'webos', fps: 30 }, output });
  const browser = createApplicationRuntime(fixture.client);
  const tvRoot = createRoot(document.getElementById('tv'));
  const browserRoot = createRoot(document.getElementById('portal'));
  let tvClosed = false;
  try {
    await act(async () => {
      tvRoot.render(
        <WebosApp
          localApplication={fixture.client}
          platform={fakeTvHost(false).client}
          portalUrl="http://tv.local:8787"
          onExit={() => {}}
        />,
      );
      browserRoot.render(<WebosWebPortal application={browser} />);
    });
    await act(async () => button('Turn lighting on', document.getElementById('tv')).click());
    assert.equal(browser.selectors.selectSnapshot(browser.store.getState()).target, 'processed');
    await act(async () => tvRoot.unmount());
    tvClosed = true;
    await act(async () =>
      browser.store.dispatch(browser.commands.configureOutput(0, { ...output, address: '192.0.2.44' })),
    );
    assert.equal(fixture.snapshot().settings.output.address, '192.0.2.44');
    assert.equal(fixture.snapshot().target, 'processed');
    assert(!fixture.calls.includes('target:off'));
  } finally {
    if (!tvClosed) await act(async () => tvRoot.unmount());
    await act(async () => browserRoot.unmount());
    dom.window.close();
  }
});
