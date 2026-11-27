import type { Processing } from '@prysm/contracts/application-reading';
import { type AdapterSide } from '@prysm/application-client';
import { useRef, type ReactNode } from 'react';
import { useParams } from 'react-router';
import { ledCount, setupPart } from './adapters';
import { CalibrateWizard } from './calibrate';
import { LightCard } from './lighting';
import { useLiveColors, usePageVisible } from './live-colors';
import { OfflineNotice, Page, PageHeading, SetupNotice } from './page';
import { useNav, usePortal } from './portal-context';
import { Detail, QuickPicture, StyleSettings } from './processing';
import { CalibrationList, CalibrationPage } from './calibrations';
import { useProcessingEditor } from './live-processing';
import { SaveStatus } from './save-status';
import { SectionEditor } from './section-editor';
import { SetupCard } from './setup-card';
import { AboutPage, SettingsPage } from './settings-page';
import { stepCopy } from './step-copy';
import { ThemeChoice } from './theme-choice';

/** The active calibration's name, or `null` without one. */
function activeCalibration(processing: Processing) {
  const active = processing.calibrations?.find(
    (calibration) => calibration.id === processing.active_calibration,
  );
  return active ? active.name : null;
}

/** Settings subpages use the same visible heading and browser title. */
function SettingsSubpage({
  page,
  name,
  children,
}: {
  page:
    | 'calibration'
    | 'calibrations'
    | 'style'
    | 'detail'
    | 'appearance'
    | 'about';
  name: string;
  children: ReactNode;
}) {
  const calibration = page === 'calibration';
  return (
    <Page
      className={`portal-page ${page}-page`}
      title={page === 'about' ? name : `Prysm ${page}`}
      name={name}
    >
      <PageHeading
        title={name}
        to={calibration ? 'calibrations' : 'settings'}
        backLabel={calibration ? 'Calibrations' : 'Settings'}
      />
      {children}
    </Page>
  );
}

/** Home's frame: wide for its two cards, with the way to Settings. */
const homeFrame = {
  className: 'portal-page home',
  title: 'Prysm',
  wide: true,
  settingsLink: true,
} as const;

const calibrateFrame = {
  className: 'portal-flow portal-flow-narrow',
  title: 'Prysm calibration',
  name: 'Calibrate',
} as const;

/** The frame a page waits in: Home's, Calibrate's or a settings page's. */
export type WaitingFrame = 'home' | 'calibrate' | 'settings';

/**
 * A page before the first state: its frame as it will be and still blocks
 * where its cards will be, so the cards take their places without moving
 * anything. `message` (empty until the wait lasts) is said in the first
 * block; the header's connection status announces the wait.
 */
export function WaitingPage({
  frame,
  message,
}: {
  frame: WaitingFrame;
  message: string;
}) {
  const said = <p className="skeleton-message">{message}</p>;
  if (frame === 'home')
    return (
      <Page {...homeFrame}>
        <div className="home-grid">
          <div className="skeleton light-skeleton">{said}</div>
          <div className="skeleton setup-skeleton" />
        </div>
      </Page>
    );
  // A settings page's title comes with its page; until then, Prysm's.
  return (
    <Page
      {...(frame === 'calibrate'
        ? calibrateFrame
        : { className: 'portal-page', title: 'Prysm' })}
    >
      <div className="skeleton page-skeleton">{said}</div>
    </Page>
  );
}

export function HomeRoute() {
  const portal = usePortal();
  const { go } = useNav();
  const { snapshot, host, online, commands, lighting, path, openSetup } =
    portal;
  const gridRef = useRef<HTMLDivElement>(null);
  const visible = usePageVisible();
  const syncing = lighting.kind === 'sync';
  useLiveColors(host.colors, online && visible && syncing, gridRef);
  const { settings } = snapshot;
  const capture =
    settings.capture && setupPart('capture', host.captures, settings.capture);
  const output =
    settings.output && setupPart('output', host.outputs, settings.output);
  return (
    <Page {...homeFrame}>
      <OfflineNotice />
      <SetupNotice />
      <div className="home-grid" ref={gridRef}>
        <LightCard
          control={snapshot.control}
          target={snapshot.target}
          lighting={lighting}
          device={snapshot.device}
          sentLedCount={output?.edges && ledCount(output.edges)}
          hasCapture={!!capture}
          hasOutput={!!output}
          output={output ? (output.lighting ?? {}) : null}
          disabled={!online}
          setTarget={commands.setTarget}
          openSetup={openSetup}
        >
          <QuickPicture
            adjustStyle={() => go('style')}
            calibrate={() => go('calibrate')}
          />
        </LightCard>
        {/* Before setup the light card's prompt says it all. */}
        {capture && output ? (
          <SetupCard
            capture={capture}
            output={output}
            path={path}
            calibration={activeCalibration(settings.processing)}
            preview={
              host.preview && {
                client: host.preview,
                online,
                target: snapshot.target,
                session: snapshot.session,
              }
            }
            openSettings={() => {
              if (path.kind === 'failed' && path.at === 'capture') go('video');
              else if (path.kind === 'failed' && path.at === 'output')
                go('lights');
              else go('settings');
            }}
          />
        ) : null}
      </div>
    </Page>
  );
}

export function SettingsRoute() {
  const portal = usePortal();
  const { snapshot } = portal;
  const live = useProcessingEditor();
  return (
    <Page
      className="portal-page settings"
      title="Prysm settings"
      name="Settings"
    >
      <PageHeading title="Settings" to="home" backLabel="Home" />
      <SaveStatus state={live.state} />
      <SetupNotice />
      {snapshot.surface_failures.length ? (
        <p className="notice warning">
          Part of Prysm’s web service stopped. The lights keep working.
        </p>
      ) : null}
      <SettingsPage live={live} />
    </Page>
  );
}

/** The video source or lights editor, on a page of its own. */
export function EditorRoute({ section }: { section: AdapterSide }) {
  const copy = stepCopy[section];
  return (
    <Page
      className="portal-page section-page"
      title={section === 'capture' ? 'Prysm video source' : 'Prysm lights'}
      name={copy.title}
    >
      <PageHeading title={copy.title} to="settings" backLabel="Settings" />
      <OfflineNotice />
      <SectionEditor section={section} />
    </Page>
  );
}

const processingPages = {
  calibrations: { name: 'Calibrations', Editor: CalibrationList },
  style: { name: 'Style', Editor: StyleSettings },
  detail: { name: 'Detail', Editor: Detail },
};

export function ProcessingRoute({
  page,
}: {
  page: keyof typeof processingPages | 'calibration';
}) {
  const live = useProcessingEditor();
  const { snapshot } = usePortal();
  const { id = '' } = useParams();
  let name: string;
  let editor: ReactNode;
  if (page === 'calibration') {
    const calibration = snapshot.settings.processing.calibrations?.find(
      (item) => item.id === id,
    );
    name = calibration ? calibration.name : 'Calibration';
    editor = <CalibrationPage live={live} id={id} />;
  } else {
    const entry = processingPages[page];
    name = entry.name;
    editor = <entry.Editor live={live} />;
  }
  return (
    <SettingsSubpage page={page} name={name}>
      <SaveStatus state={live.state} />
      {editor}
    </SettingsSubpage>
  );
}

export function AppearanceRoute() {
  return (
    <SettingsSubpage page="appearance" name="Appearance">
      <p className="save-status" data-phase="local">
        Kept in this browser
      </p>
      <ThemeChoice />
    </SettingsSubpage>
  );
}

export function AboutRoute() {
  const { snapshot } = usePortal();
  return (
    <SettingsSubpage page="about" name="About this Prysm">
      <p className="save-status" data-phase="local" />
      <AboutPage snapshot={snapshot} />
    </SettingsSubpage>
  );
}

export function CalibrateRoute() {
  return (
    <Page {...calibrateFrame}>
      <OfflineNotice />
      <CalibrateWizard />
    </Page>
  );
}
