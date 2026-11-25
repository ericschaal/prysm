/**
 * Demo of a TV bundle on a desktop browser (`npm run dev:webos`, then
 * `/demo.html`): the real Luna client over an in-memory supervisor and an
 * in-memory portal. Query flags: `offline` (service not set up), `refused`
 * (the portal answers but refuses this origin), `unreachable`, `old` (a TV
 * that cannot play the calibration patterns), `nolights` (a first run: the
 * portal has no lights yet; `demo.addLights()` stands in for its setup
 * wizard), `noaddress` (slim: no Prysm address saved yet), `hold` (the
 * supervisor answers nothing until `demo.supervisor.release()`: the screen
 * before the first state). Selects at the bottom right put
 * the supervisor in one of its states (asking after a failure, waiting
 * for video, …) and set or clear the calibration pattern, as the
 * phone does.
 * Never part of a production build: `index.html` does not load it.
 */
import type { ReactElement } from 'react';
import { createRoot } from 'react-dom/client';
import { ApplicationError } from '@prysm/application-client';
import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/application';
import {
  isEnabled,
  lunaTvService,
  type Fix,
  type Flavor,
} from '@prysm/webos-platform';
import type { PortalOptions, TvAppProps } from '@prysm/tv-ui';
import { fakeApplicationPort } from './application';
import { fakeTvSupervisor, type FakeChange } from './luna';
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
      flavor === 'slim' && !flags.has('noaddress')
        ? { prysm_address: 'http://192.168.1.20:8787' }
        : {},
  });
  if (flags.has('hold')) supervisor.hold();
  const output = {
    kind: 'wled',
    address: '192.168.1.50',
    leds: { top: 96, right: 54, bottom: 96, left: 54 },
  };
  const capture =
    flavor === 'full'
      ? { kind: 'webos', width: 640, height: 360, fps: 30 }
      : { kind: 'remote' };
  const application = fakeApplicationPort<CaptureSettings, OutputSettings>(
    flags.has('nolights') ? { capture } : { capture, output },
  );
  /** What the portal's setup wizard does when it saves (with Save only). */
  const addLights = () =>
    application.publish({
      settings: { ...application.snapshot().settings, output },
    });
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
                  message: 'Can’t reach Prysm.',
                }),
              ),
          }
        : application.client,
    probe: () => Promise.resolve(flags.has('refused')),
  };
  // A worker that starts capturing a moment after it is enabled; the full
  // worker's engine follows the supervisor's target and reports portal changes.
  const targetOf = () => {
    const facts = supervisor.facts();
    return facts.flavor === 'full' ? facts.target : null;
  };
  let sent = targetOf();
  // The full worker reports its engine's failure, as the real one does.
  if (flavor === 'full')
    application.watch((snapshot) => supervisor.report(snapshot));
  setInterval(() => {
    const facts = supervisor.facts();
    const capturing =
      isEnabled(supervisor.state()) &&
      facts.worker.phase === 'running' &&
      !facts.problem;
    if (facts.capturing !== capturing)
      supervisor.publish({ capturing, fps: capturing ? 30 : null });
    if (facts.flavor !== 'full') return;
    const target = application.snapshot().target;
    if (facts.target !== sent) void application.client.setTarget(facts.target);
    else if (target !== facts.target && target !== 'unknown')
      supervisor.publish({
        target,
        capturing: target === 'screen_sync',
        fps: target === 'screen_sync' ? 30 : null,
      });
    sent = targetOf();
  }, 500);
  Object.assign(window, { demo: { supervisor, application, addLights } });
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
  // Lighting (full) or capture (slim) on, in the package's own terms.
  const on: FakeChange =
    flavor === 'full' ? { target: 'screen_sync' } : { enabled: true };
  // No worker: the service asks (full: the modes go to it).
  const asking = (fix: Fix, message: string): FakeChange => ({
    worker: {
      phase: 'asking',
      problem: { asked: { step: 'ask', fix }, message },
    },
    capturing: false,
    fps: null,
  });
  const states: Record<string, FakeChange> = {
    'TV: as it is': {},
    'TV: asking, try again': asking(
      'try_again',
      'again after a restart: worker killed by signal 9',
    ),
    'TV: asking, set up': asking('set_up', 'open capture: permission denied'),
    'TV: asking, restart the TV': asking(
      'restart_tv',
      'worker not reaped 8 s after its kill',
    ),
    // Full: the engine's session failure, which the worker reports (below).
    'TV: failure':
      flavor === 'full'
        ? {}
        : asking('try_again', 'Capture stopped: no video frames'),
    'TV: waiting for video': {
      ...on,
      video: 'unavailable',
      capturing: false,
      fps: null,
    },
    'TV: asleep': { ...on, power: 'asleep', capturing: false },
    'TV: stopping': { worker: { phase: 'stopping' } },
    ...(flavor === 'slim'
      ? {
          'TV: waiting for Prysm': {
            enabled: true,
            worker: { phase: 'running' },
            uplink: { state: 'waiting', message: 'Connection refused' },
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
            enabled: true,
            worker: { phase: 'running' },
            uplink: {
              state: 'encoder_failed',
              message: 'Hardware encoder busy',
            },
            capturing: false,
            fps: null,
          },
          'TV: Prysm of another version': {
            enabled: true,
            worker: { phase: 'running' },
            uplink: {
              state: 'version_differs',
              message:
                'the capture stream versions differ (Prysm: 7, this end: 6); update the older one',
            },
            capturing: false,
            fps: null,
          },
          'TV: another TV took over': {
            enabled: true,
            worker: { phase: 'running' },
            uplink: {
              state: 'replaced',
              message:
                'the stream to ws://prysm.local:8787/api/v1/capture/stream failed: another sender replaced this one at Prysm',
            },
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
    if (flavor === 'full' && tvState.value === 'TV: failure') {
      // As on a TV: screen sync asked, then its session failed.
      application.publish({
        target: 'screen_sync',
        session: {
          state: 'stopped',
          failure: {
            kind: 'adapter',
            section: 'capture',
            message: 'Capture stopped: no video frames',
          },
        },
      });
      return;
    }
    const change = states[tvState.value];
    const facts = supervisor.facts();
    const running = facts.flavor === 'full' || facts.enabled;
    const back: FakeChange = {
      problem: null,
      power: 'active',
      video: 'available',
      worker: { phase: running ? 'running' : 'stopped' },
      ...(facts.flavor === 'slim' ? { paused: false, uplink: null } : {}),
    };
    supervisor.publish(change && Object.keys(change).length ? change : back);
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
