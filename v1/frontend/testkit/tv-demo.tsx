/**
 * Demo of a TV bundle on a desktop browser (`npm run dev:webos`, then
 * `/demo.html`): the real Luna client over an in-memory supervisor and an
 * in-memory portal. Query flags: `offline` (service not set up), `refused`
 * (the portal answers but refuses this origin), `unreachable`, `old` (a TV
 * that cannot play the calibration patterns). Selects at the bottom right put
 * the supervisor in one of its states (a failure with a retry countdown,
 * waiting for video, …) and set or clear the calibration pattern, as the
 * phone does.
 * Never part of a production build: `index.html` does not load it.
 */
import type { ReactElement } from 'react';
import { createRoot } from 'react-dom/client';
import { ApplicationError } from '@prysm/application-client';
import { lunaTvService, type Flavor, type State } from '@prysm/webos-platform';
import type { PortalOptions, TvAppProps } from '@prysm/tv-ui';
import { fakeApplicationPort } from './application';
import { fakeTvSupervisor } from './luna';
import {
  msePatternPlayer,
  patternNames,
  type PatternPlayer,
} from '../packages/tv-ui/patterns';

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
  // `old`: a TV that cannot play the patterns. A browser without H.264
  // (e.g. Chromium) shows each pattern as a plain colour instead.
  const colours: PatternPlayer = {
    supported: true,
    play(video, pattern) {
      const paint = () => {
        const name = pattern();
        video.style.background = name.startsWith('gray')
          ? '#c0c0c0'
          : (/^(red|green|blue|cyan|magenta|yellow|white)/.exec(name)?.[0] ??
            '#808080');
      };
      paint();
      const timer = setInterval(paint, 250);
      return () => clearInterval(timer);
    },
  };
  let patterns = msePatternPlayer.supported ? msePatternPlayer : colours;
  if (flags.has('old')) patterns = { supported: false, play: () => () => {} };
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
      state.enabled && state.worker.phase === 'running' && !state.failure;
    if (state.capturing !== capturing)
      supervisor.publish({ capturing, fps: capturing ? 30 : null });
    if (flavor !== 'full') return;
    const target = application.snapshot().target;
    if (state.lighting !== lighting && state.lighting)
      void application.client.setTarget(state.lighting);
    else if (target !== state.lighting)
      supervisor.publish({
        lighting: target,
        enabled: target !== 'off',
        capturing: target === 'processed',
        fps: target === 'processed' ? 30 : null,
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
  // The supervisor's states the UI distinguishes, to look at each one.
  const states: Record<string, Partial<State>> = {
    'TV: as it is': {},
    'TV: failure, retry countdown': {
      worker: { phase: 'retrying', seconds: 30 },
      failure: { kind: 'other', message: 'Capture stopped: no video frames' },
      capturing: false,
      fps: null,
    },
    'TV: failure': {
      failure: { kind: 'other', message: 'Capture stopped: no video frames' },
      capturing: false,
      fps: null,
    },
    'TV: waiting for video': {
      enabled: true,
      video: 'unavailable',
      capturing: false,
      fps: null,
    },
    'TV: asleep': { enabled: true, power: 'asleep', capturing: false },
    'TV: stopping': { worker: { phase: 'stopping' } },
    ...(flavor === 'slim'
      ? {
          'TV: waiting for Prysm': {
            enabled: true,
            worker: { phase: 'running' },
            failure: { kind: 'receiver', message: 'Connection refused' },
            capturing: false,
            fps: null,
          },
          'TV: paused by Prysm': {
            enabled: true,
            paused: true,
            capturing: false,
            fps: null,
          },
          'TV: encoder failure': {
            worker: { phase: 'retrying', seconds: 10 },
            failure: { kind: 'encoder', message: 'Hardware encoder busy' },
            capturing: false,
            fps: null,
          },
        }
      : {}),
  };
  const tvState = document.createElement('select');
  tvState.title = 'Demo: the TV supervisor state';
  tvState.setAttribute(
    'style',
    'position:fixed;right:200px;bottom:8px;z-index:1;font:14px sans-serif',
  );
  for (const name of Object.keys(states)) tvState.add(new Option(name, name));
  tvState.onchange = () => {
    const change = states[tvState.value];
    const { enabled } = supervisor.state();
    supervisor.publish(
      change && Object.keys(change).length
        ? change
        : {
            failure: null,
            paused: false,
            power: 'active',
            video: 'available',
            worker: {
              phase: flavor === 'full' || enabled ? 'running' : 'stopped',
            },
          },
    );
  };
  document.body.append(tvState);
  createRoot(root).render(
    <App
      service={lunaTvService(supervisor.bridge)}
      portal={portal}
      patterns={patterns}
      onExit={() => {
        document.title = 'Prysm (demo: closed)';
        root.textContent = 'The app would close now; lighting is unchanged.';
      }}
    />,
  );
}
