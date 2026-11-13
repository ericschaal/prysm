import './state.test.jsx';
import './http.test.jsx';
import './portal.test.jsx';
import './setup.test.jsx';
import './preview.test.jsx';
import './colors.test.jsx';
import './profiles.test.jsx';
import './tv.test.jsx';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { createApplicationRuntime } from '@prysm/application-state';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { lunaTvService } from '@prysm/webos-platform';
import { WebosTvApp } from '../hosts/webos/tv-ui/app';
import { WebosSlimTvApp } from '../hosts/webos-slim/tv-ui/app';
import { fakeApplicationPort } from '../testkit/application';
import { fakeTvSupervisor } from '../testkit/luna';
import { mountDom, sleep } from './dom';

const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};

const tvProps = (flavor, fixture, supervisor) => ({
  service: lunaTvService(supervisor.bridge),
  portal: { client: () => fixture.client, supported: true },
  onExit: () => {},
});
const slimPreferences = { instance: 'http://192.0.2.20:8787' };

test('TV assemblies show their package: Luna for the TV, HTTP for the lights', async () => {
  for (const flavor of ['full', 'slim']) {
    const dom = mountDom();
    // The TV app subscribes only while the page is visible.
    Object.defineProperty(document, 'hidden', {
      value: false,
      configurable: true,
    });
    const fixture = fakeApplicationPort(
      flavor === 'full'
        ? { capture: { kind: 'webos' }, output }
        : { capture: { kind: 'remote' }, output },
    );
    const supervisor = fakeTvSupervisor({
      flavor,
      preferences: flavor === 'slim' ? slimPreferences : {},
    });
    const Host = flavor === 'full' ? WebosTvApp : WebosSlimTvApp;
    const root = createRoot(document.getElementById('root'));
    try {
      await act(async () =>
        root.render(<Host {...tvProps(flavor, fixture, supervisor)} />),
      );
      await act(async () => sleep(5));
      // Both packages point to the portal that holds capture settings.
      assert.equal(
        document.getElementById('portal-address').querySelector('code')
          .textContent,
        flavor === 'full'
          ? 'http://192.168.1.5:8787/'
          : slimPreferences.instance,
      );
      assert.equal(
        document.getElementById('capture'),
        null,
        'capture is set in the portal only',
      );
      assert.equal(!!document.getElementById('instance'), flavor === 'slim');
      await act(async () => document.getElementById('mode-processed').click());
      assert.equal(fixture.snapshot().target, 'processed');
      await act(async () => root.unmount());
      // Leaving the UI releases the streams and never turns lighting off.
      assert(fixture.calls.includes('unsubscribe'));
      assert.equal(fixture.snapshot().target, 'processed');
      assert.equal(supervisor.subscribers(), 0);
    } finally {
      dom.window.close();
    }
  }
});

test('embedded TV and browser portal share one application; either may close', async () => {
  const dom = mountDom('<div id="tv"></div><div id="portal"></div>');
  Object.defineProperty(document, 'hidden', {
    value: false,
    configurable: true,
  });
  const fixture = fakeApplicationPort({
    capture: { kind: 'webos', fps: 30 },
    output,
  });
  const browser = createApplicationRuntime(fixture.client);
  const tvRoot = createRoot(document.getElementById('tv'));
  const browserRoot = createRoot(document.getElementById('portal'));
  let tvClosed = false;
  try {
    await act(async () => {
      tvRoot.render(
        <WebosTvApp
          {...tvProps('full', fixture, fakeTvSupervisor({ flavor: 'full' }))}
        />,
      );
      browserRoot.render(<WebosWebPortal application={browser} />);
    });
    await act(async () => sleep(5));
    await act(async () => document.getElementById('mode-processed').click());
    assert.equal(
      browser.selectors.selectSnapshot(browser.store.getState()).target,
      'processed',
    );
    await act(async () => tvRoot.unmount());
    tvClosed = true;
    await act(async () =>
      browser.store.dispatch(
        browser.commands.configureOutput(0, {
          ...output,
          address: '192.0.2.44',
        }),
      ),
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

// Portal calibration wizard.
import './calibrate.test.jsx';
