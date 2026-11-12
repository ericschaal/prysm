import './state.test.jsx';
import './http.test.jsx';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { createApplicationRuntime } from '@prysm/application-state';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { WebosApp } from '../hosts/webos/tv-ui/app';
import { WebosSlimApp } from '../hosts/webos-slim/tv-ui/app';
import { fakeApplicationPort } from '../testkit/application';
import { fakeTvHost } from '../testkit/tv';

const output = { kind: 'wled', address: '192.0.2.1', leds: { top: 10, left: 4 } };

function mountDom(html = '<div id="root"></div>') {
  const dom = new JSDOM(html);
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    FormData: dom.window.FormData,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  return dom;
}
const submit = (dom, form) =>
  act(async () =>
    form.dispatchEvent(
      new dom.window.Event('submit', { bubbles: true, cancelable: true }),
    ),
  );
const button = (text, scope = document) =>
  [...scope.querySelectorAll('button')].find((item) => item.textContent === text);
const alert = () => document.querySelector('[role="alert"]')?.textContent;

test('both browser hosts mount their panels and replace one section at a time', async () => {
  for (const [Host, capture, heading, field, value, other] of [
    [HeadlessWebPortal, { kind: 'device', path: '/dev/video0', fps: 30 }, 'Video device capture', 'path', '/dev/video1', 'webOS capture'],
    [HeadlessWebPortal, { kind: 'remote', listen: '0.0.0.0:9000' }, 'Remote capture', 'listen', '0.0.0.0:9001', 'webOS capture'],
    [WebosWebPortal, { kind: 'webos', fps: 30 }, 'webOS capture', 'fps', '60', 'Video device capture'],
  ]) {
    const dom = mountDom();
    const fixture = fakeApplicationPort({ capture, output });
    const application = createApplicationRuntime(fixture.client);
    const root = createRoot(document.getElementById('root'));
    try {
      await act(async () => root.render(<Host application={application} />));
      assert(document.body.textContent.includes(heading));
      assert(document.body.textContent.includes('WLED output'));
      assert(!document.body.textContent.includes(other));

      document.querySelector(`[name="${field}"]`).value = value;
      await submit(dom, document.querySelector('form'));
      const saved = fixture.snapshot();
      assert.equal(saved.revision, 1);
      assert.equal(saved.settings.capture[field], field === 'fps' ? 60 : value);
      assert.equal(saved.settings.capture.kind, capture.kind);
      assert.deepEqual(saved.settings.output, output);
      assert(document.body.textContent.includes('Settings revision: 1'));

      document.querySelector('[name="address"]').value = '192.0.2.2';
      await submit(dom, document.querySelectorAll('form')[1]);
      assert.equal(fixture.snapshot().settings.output.address, '192.0.2.2');
      assert.deepEqual(fixture.snapshot().settings.output.leds, output.leds);
      assert.equal(fixture.snapshot().settings.capture[field], saved.settings.capture[field]);

      fixture.failNext({ kind: 'invalid', message: 'a unicast device address is required', field: 'output.address' });
      await submit(dom, document.querySelectorAll('form')[1]);
      assert.equal(alert(), 'output.address: a unicast device address is required');
      assert.equal(fixture.snapshot().revision, 2);
    } finally {
      await act(async () => root.unmount());
      dom.window.close();
    }
  }
});

test('the portal shows lighting state, failures and a faulted session', async () => {
  const dom = mountDom();
  const fixture = fakeApplicationPort({ output });
  const application = createApplicationRuntime(fixture.client);
  const root = createRoot(document.getElementById('root'));
  try {
    await act(async () => root.render(<HeadlessWebPortal application={application} />));
    await act(async () => button('Turn lighting on').click());
    assert(document.body.textContent.includes('Target: processed · Session: running'));
    await act(async () =>
      fixture.publish({
        session: { state: 'faulted', phase: 'closing_source' },
        failure: { kind: 'faulted', message: 'runtime is faulted: session did not finish closing the source' },
      }),
    );
    assert(document.body.textContent.includes('faulted while closing source; restart Prysm'));
    assert(alert().startsWith('Lighting failed: runtime is faulted'));
    fixture.failNext({ kind: 'faulted', message: 'runtime is faulted' });
    await act(async () => button('Turn lighting off').click());
    assert.equal(
      [...document.querySelectorAll('[role="alert"]')].at(-1).textContent,
      'runtime is faulted',
    );
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
  }
});

test('a stale edit is refused locally instead of overwriting newer settings', async () => {
  const dom = mountDom();
  const fixture = fakeApplicationPort({ capture: { kind: 'device', path: '/dev/video0' }, output });
  const application = createApplicationRuntime(fixture.client);
  const root = createRoot(document.getElementById('root'));
  try {
    await act(async () => root.render(<HeadlessWebPortal application={application} />));
    // Another client saves first; this form still shows revision 0.
    await fixture.client.replaceSettings(0, { ...fixture.snapshot().settings, output: { ...output, address: '192.0.2.9' } });
    await assert.rejects(
      application.store.dispatch(application.commands.configureCapture(0, { kind: 'device', path: '/dev/video5' })),
    );
    assert.equal(application.store.getState().lastCommand.failure.kind, 'stale_revision');
    assert.equal(fixture.snapshot().settings.capture.path, '/dev/video0');
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
  }
});

test('the headless capture selector switches kinds without sending until saved', async () => {
  const dom = mountDom();
  const fixture = fakeApplicationPort({ capture: { kind: 'device', path: '/dev/video0', fps: 30 }, output });
  const application = createApplicationRuntime(fixture.client);
  const root = createRoot(document.getElementById('root'));
  try {
    await act(async () => root.render(<HeadlessWebPortal application={application} />));
    const select = document.querySelector('[name="kind"]');
    select.value = 'remote';
    await act(async () => select.dispatchEvent(new dom.window.Event('change', { bubbles: true })));
    assert(document.body.textContent.includes('Remote capture'));
    assert.equal(fixture.snapshot().settings.capture.kind, 'device');
    assert.equal(document.querySelector('[name="listen"]').value, '');
    document.querySelector('[name="listen"]').value = '0.0.0.0:9000';
    await submit(dom, document.querySelector('form'));
    assert.deepEqual(fixture.snapshot().settings.capture, { kind: 'remote', listen: '0.0.0.0:9000' });
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
  }
});

test('TV assemblies route lighting to the application and platform commands to the TV', async () => {
  for (const embedded of [true, false]) {
    const dom = mountDom();
    const fixture = fakeApplicationPort({ output });
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
