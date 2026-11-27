import {
  lightingSituation,
  plainFailure,
  situationFailure,
  terminalStreamFailure,
} from '@prysm/application-client';
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  createHashRouter,
  Outlet,
  RouterProvider,
  ScrollRestoration,
  useLocation,
  useMatch,
  type RouteObject,
} from 'react-router';
import { useConnection } from '@prysm/application-react';
import { readSection, type AdapterSection } from './adapters';
import { useDeveloper } from './developer';
import { useLingering } from './lingering';
import { OfflineNotice, Page } from './page';
import {
  AboutRoute,
  AppearanceRoute,
  CalibrateRoute,
  EditorRoute,
  HomeRoute,
  ProcessingRoute,
  SettingsRoute,
  WaitingPage,
  type WaitingFrame,
} from './pages';
import {
  ChromeContext,
  HostContext,
  PortalContext,
  useHost,
  type Chrome,
  type Host,
  type Portal,
  type WebPortalProps,
} from './portal-context';
import type { DeletedCalibration } from './calibrations';
import { pages, showsHome } from './routes';
import { SetupWizard } from './setup';
import { useReplaced } from './replaced';
import { PortalError } from './update';
import { pathOf } from './section-state';
import './tokens.css';
import './portal.css';
import './flows.css';

/** Delay before the portal displays a connection warning. */
export const connectionGraceMs = 1000;

/**
 * The frame of every page: the state, setup when it takes over, and the page
 * the hash names. Each page is its own: a new one mounts in place of the
 * last (keyed by its address), so nothing a page holds carries over to the
 * next. The router keeps each history entry's scroll place: a new page
 * starts at its top, going back returns to where it was.
 */
function PortalFrame() {
  const host = useHost();
  const { application } = host;
  const connection = useConnection(application);
  // The last state (and every draft) stays shown while reconnecting. Every
  // page meets the saved sections as their kinds' types have them.
  const sent = connection.snapshot;
  const snapshot = useMemo(() => {
    if (!sent) return sent;
    const { capture, output } = sent.settings;
    return {
      ...sent,
      settings: {
        ...sent.settings,
        ...(capture && { capture: readSection(host.captures, capture) }),
        ...(output && { output: readSection(host.outputs, output) }),
      },
    };
  }, [sent, host.captures, host.outputs]);
  const online = connection.status === 'connected';
  // Setup opens by itself when the first state lacks capture or lights.
  const [setup, setSetup] = useState<'pending' | 'open' | 'closed'>('pending');
  if (setup === 'pending' && snapshot)
    setSetup(
      snapshot.settings.capture && snapshot.settings.output ? 'closed' : 'open',
    );
  const [setupNotice, setSetupNotice] = useState('');
  const location = useLocation();
  const calibrating = useMatch(pages.calibrate) !== null;
  const [previous, setPrevious] = useState<Portal['previous']>({
    capture: null,
    output: null,
  });
  const [deletedCalibrations, setDeletedCalibrations] = useState<
    Record<string, DeletedCalibration>
  >({});
  const noticeRef = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    if (setupNotice) noticeRef.current?.focus();
  }, [setupNotice]);

  // Disabled at once while offline, but said only once it lasts.
  const outOfTouch = useLingering(!online, connectionGraceMs, 0);
  const stopped = terminalStreamFailure(connection.error)?.message ?? null;
  let connectionLabel = 'Offline';
  if (online) connectionLabel = 'Connected';
  else if (connection.status === 'connecting') connectionLabel = 'Connecting…';
  else if (snapshot && !stopped) connectionLabel = 'Reconnecting…';
  const chrome: Chrome = {
    connectionLabel,
    outOfTouch,
    stopped,
    developer: useDeveloper(),
    updated: useReplaced(online, host.replaced),
  };

  // Normally the page comes with the state; without it (a dev server, or a
  // state the page could not read) the page waits in its route's frame, and
  // says so only once the wait lasts. The first state may open Setup in its
  // place (capture or lights missing), which only that state can tell.
  if (!snapshot) {
    let frame: WaitingFrame = 'settings';
    if (calibrating) frame = 'calibrate';
    else if (showsHome(location.pathname)) frame = 'home';
    return (
      <ChromeContext value={chrome}>
        <WaitingPage
          frame={frame}
          message={
            outOfTouch
              ? (connection.error?.message ?? 'Connecting to Prysm…')
              : ''
          }
        />
      </ChromeContext>
    );
  }

  const lighting = lightingSituation(snapshot);
  const path = pathOf(lighting);
  const { commands } = application;
  const portal: Portal = {
    host,
    snapshot,
    online,
    commands,
    lighting,
    path,
    problemAt: (section) => {
      const failure = situationFailure(lighting);
      return path.kind === 'failed' && path.at === section && failure
        ? plainFailure(failure, snapshot.target, 'Device effects')
        : null;
    },
    setupNotice,
    noticeRef,
    openSetup: () => {
      setSetupNotice('');
      setSetup('open');
    },
    previous,
    setPrevious,
    deletedCalibrations,
    setDeletedCalibrations,
  };
  return (
    <ChromeContext value={chrome}>
      <PortalContext value={portal}>
        <ScrollRestoration />
        {/* Setup takes over whatever the hash says, except Calibrate. */}
        {setup === 'open' && !calibrating ? (
          <Page className="portal-flow" title="Prysm setup" name="Setup">
            <OfflineNotice />
            <SetupWizard
              close={(notice) => {
                setSetup('closed');
                setSetupNotice(notice);
              }}
            />
          </Page>
        ) : (
          <Outlet key={location.pathname} />
        )}
      </PortalContext>
    </ChromeContext>
  );
}

/** Every page, by its address under the hash; anything else is Home. */
const portalRoutes: RouteObject[] = [
  {
    element: <PortalFrame />,
    errorElement: <PortalError />,
    children: [
      { path: pages.home, element: <HomeRoute /> },
      { path: pages.settings, element: <SettingsRoute /> },
      { path: pages.video, element: <EditorRoute section="capture" /> },
      { path: pages.lights, element: <EditorRoute section="output" /> },
      ...(['calibrations', 'calibration', 'style', 'detail'] as const).map(
        (page) => ({
          path: pages[page],
          element: <ProcessingRoute key={page} page={page} />,
        }),
      ),
      { path: pages.appearance, element: <AppearanceRoute /> },
      { path: pages.about, element: <AboutRoute /> },
      { path: pages.calibrate, element: <CalibrateRoute /> },
      { path: '*', element: <HomeRoute /> },
    ],
  },
];

/**
 * The browser portal. Home holds the light card (power, mode, brightness,
 * calibration) and a summary of the setup with the live preview; Settings
 * (`#/settings`) holds the picture tuning, the capture and lights forms and
 * the technical details; the Calibrate page is at
 * `#/calibrate`; the guided setup takes over while capture or lights are
 * missing. The host chooses which capture and output kinds exist.
 */
export function WebPortal<C extends AdapterSection, O extends AdapterSection>(
  props: WebPortalProps<C, O>,
) {
  const [router] = useState(() => createHashRouter(portalRoutes));
  // Above the router: a page that fails keeps the state stream.
  const { application } = props;
  useEffect(() => application.connect(), [application]);
  // The pages are not generic: they pass each section back only to the
  // host's own commands and adapters, so its kinds can be forgotten here.
  return (
    <HostContext value={props as unknown as Host}>
      <RouterProvider router={router} />
    </HostContext>
  );
}
