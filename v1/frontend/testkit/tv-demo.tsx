/**
 * Demo of a TV bundle on a desktop browser (`npm run dev:webos`, then
 * `/demo.html`): the real Luna client over an in-memory supervisor and an
 * in-memory portal. Query flags: `offline` (service not set up), `refused`
 * (the portal answers but refuses this origin), `unreachable`. A select at
 * the bottom right sets or clears the calibration pattern, as the phone does.
 * Never part of a production build: `index.html` does not load it.
 */
import type { ReactElement } from 'react';
import { createRoot } from 'react-dom/client';
import { ApplicationError } from '@prysm/application-client';
import { lunaTvService, type Flavor } from '@prysm/webos-platform';
import type { PortalOptions, TvAppProps } from '@prysm/tv-ui';
import { fakeApplicationPort } from './application';
import { fakeTvSupervisor } from './luna';
import { patternNames } from '../packages/tv-ui/patterns';

export function runDemo(
  flavor: Flavor,
  App: (props: Omit<TvAppProps, 'flavor'>) => ReactElement,
) {
  const flags = new URLSearchParams(location.search);
  const supervisor = fakeTvSupervisor({
    flavor,
    offline: flags.has('offline'),
    delayMs: 150,
    preferences:
      flavor === 'slim'
        ? { instance: 'http://192.168.1.20:8787', receiver: null }
        : {},
  });
  const application = fakeApplicationPort<unknown, unknown>(
    flavor === 'full'
      ? {
          capture: { kind: 'webos', width: 640, height: 360, fps: 30 },
          output: {
            kind: 'wled',
            address: '192.168.1.50',
            leds: { top: 96, right: 54, bottom: 96, left: 54 },
          },
        }
      : {
          capture: { kind: 'remote', listen: '0.0.0.0:7380' },
          output: {
            kind: 'wled',
            address: '192.168.1.50',
            leds: { top: 96, right: 54, bottom: 96, left: 54 },
          },
        },
  );
  const blocked = flags.has('refused') || flags.has('unreachable');
  const portal: PortalOptions = {
    client: () =>
      blocked
        ? {
            ...application.client,
            observe: () =>
              Promise.reject(
                new ApplicationError({
                  kind: 'unavailable',
                  message: 'Cannot reach the portal.',
                }),
              ),
          }
        : application.client,
    probe: () => Promise.resolve(flags.has('refused')),
  };
  // A worker that starts capturing a moment after it is enabled; the full
  // worker's engine follows the supervisor's target and reports portal changes.
  let lighting = supervisor.state().lighting;
  setInterval(() => {
    const state = supervisor.state();
    const capturing =
      state.enabled && state.worker === 'running' && !state.failure;
    if (state.capturing !== capturing) supervisor.publish({ capturing });
    if (flavor !== 'full') return;
    const target = application.snapshot().target;
    if (state.lighting !== lighting && state.lighting)
      void application.client.setTarget(state.lighting);
    else if (target !== state.lighting)
      supervisor.publish({
        lighting: target,
        enabled: target !== 'off',
        capturing: target === 'processed',
      });
    lighting = supervisor.state().lighting;
  }, 500);
  Object.assign(window, { demo: { supervisor, application } });
  const root = document.getElementById('root');
  if (!root) throw new Error('Missing TV root.');
  // Calibration: plays the phone's part (choose Calibrate colours on the TV first).
  const phone = document.createElement('select');
  phone.title = 'Demo: the pattern the phone shows';
  phone.setAttribute(
    'style',
    'position:fixed;right:8px;bottom:8px;z-index:1;font:14px sans-serif',
  );
  for (const name of ['', ...patternNames])
    phone.add(new Option(name ? `Phone: ${name}` : 'Phone: no pattern', name));
  phone.onchange = () => {
    const pattern = patternNames.find((name) => name === phone.value);
    application.client
      .setCalibration(pattern ? { pattern, uncorrected: false } : null)
      .catch((error: unknown) => console.warn(error));
  };
  document.body.append(phone);
  createRoot(root).render(
    <App
      service={lunaTvService(supervisor.bridge)}
      portal={portal}
      onExit={() => {
        document.title = 'Prysm (demo: closed)';
        root.textContent = 'The app would close now; lighting is unchanged.';
      }}
    />,
  );
}
