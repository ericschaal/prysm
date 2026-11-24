import type { Processing } from '@prysm/contracts/application';
import {
  situationFailure,
  type AdapterSide,
  type ApplicationSnapshot,
  type DiscoveryClient,
  type LightingSituation,
  type LightingTarget,
} from '@prysm/application-client';
import { useRef, type ReactNode, type RefObject } from 'react';
import { useParams } from 'react-router';
import {
  adapterFor,
  ledCount,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import { CalibrateWizard } from './calibrate';
import { CaptureModesContext } from './capture-modes';
import { DiscoveryContext } from './discovery-context';
import { LightCard } from './lighting';
import { useLiveColors, usePageVisible } from './live-colors';
import { OfflineNotice, Page, PageHeading, SetupNotice } from './page';
import { useNav, usePortal } from './portal-context';
import type { Place } from './routes';
import type { PreviewSource } from './preview';
import { Detail, QuickPicture, StyleSettings } from './processing';
import { CalibrationList, CalibrationPage } from './calibrations';
import { SaveStatus, SaveStatusProvider } from './save-status';
import { SectionEditor } from './section-editor';
import { SetupCard, type PathState } from './setup-card';
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

/** A page under Settings that saves as it changes: its title and status. */
function LivePage({
  className,
  title,
  name,
  heading,
  to = 'settings',
  backLabel = 'Settings',
  note,
  children,
}: {
  className: string;
  title: string;
  name: string;
  heading: string;
  to?: Place;
  backLabel?: string;
  /** Instead of the save status, for a page with nothing to save. */
  note?: string;
  children: ReactNode;
}) {
  const { online } = usePortal();
  return (
    <Page className={`portal-page ${className}`} title={title} name={name}>
      <SaveStatusProvider>
        <PageHeading title={heading} to={to} backLabel={backLabel} />
        {/* Choices kept in this browser have nothing to save: said so, in
            the same place, so every page starts alike. */}
        {note !== undefined ? (
          <p className="save-status" data-phase="local">
            {note}
          </p>
        ) : (
          <SaveStatus online={online} />
        )}
        {children}
      </SaveStatusProvider>
    </Page>
  );
}

/** The processing a live page edits, and how it saves and tries. */
function useLiveProps() {
  const { snapshot, online, saveProcessing, tryProcessing } = usePortal();
  return {
    processing: snapshot.settings.processing,
    revision: snapshot.revision,
    disabled: !online,
    save: saveProcessing,
    tryProcessing,
  };
}

/** Home's frame: wide for its two cards, with the way to Settings. */
const homeFrame = {
  className: 'portal-page home',
  title: 'Prysm',
  wide: true,
  settingsLink: true,
} as const;

/** Calibrate's frame: a flow in the narrow column. */
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
  const nav = useNav();
  const { snapshot, host, online, dispatch, commands } = portal;
  // The glow takes the LEDs' colours while screen sync runs and is seen.
  const gridRef = useRef<HTMLDivElement>(null);
  const visible = usePageVisible();
  const syncing = portal.lighting.kind === 'sync';
  useLiveColors(host.colors, online && visible && syncing, gridRef);
  return (
    <Page {...homeFrame}>
      <OfflineNotice />
      <SetupNotice />
      <HomePage
        snapshot={snapshot}
        captures={host.captures}
        outputs={host.outputs}
        discovery={host.discovery ?? null}
        preview={
          host.preview
            ? {
                client: host.preview,
                online,
                target: snapshot.target,
                session: snapshot.session,
              }
            : undefined
        }
        gridRef={gridRef}
        online={online}
        lighting={portal.lighting}
        path={portal.path}
        setTarget={(target) => dispatch(commands.setTarget(target))}
        save={portal.saveProcessing}
        tryProcessing={portal.tryProcessing}
        openSetup={portal.openSetup}
        calibrate={() => nav.go('calibrate')}
        go={nav.go}
      />
    </Page>
  );
}

export function SettingsRoute() {
  const portal = usePortal();
  const nav = useNav();
  const { snapshot, host, online } = portal;
  return (
    <Page
      className="portal-page settings"
      title="Prysm settings"
      name="Settings"
    >
      <SaveStatusProvider>
        <PageHeading title="Settings" to="home" backLabel="Home" />
        <SaveStatus online={online} />
        <SetupNotice />
        {snapshot.surface_failures.length ? (
          <p className="notice warning">
            Part of Prysm’s web service stopped. The lights keep working.
          </p>
        ) : null}
        <SettingsPage
          captures={host.captures}
          outputs={host.outputs}
          snapshot={snapshot}
          online={online}
          discovery={host.discovery ?? null}
          problemAt={portal.problemAt}
          save={portal.saveProcessing}
          tryProcessing={portal.tryProcessing}
          open={(place) =>
            place === 'setup' ? portal.openSetup() : nav.go(place)
          }
        />
      </SaveStatusProvider>
    </Page>
  );
}

/** The video source or lights editor, on a page of its own. */
export function EditorRoute({ section }: { section: AdapterSide }) {
  const portal = usePortal();
  const nav = useNav();
  const { snapshot, host, online, dispatch, commands } = portal;
  const { settings } = snapshot;
  const copy = stepCopy[section];
  const shared = {
    revision: snapshot.revision,
    disabled: !online,
    live: {
      online,
      target: snapshot.target,
      path: portal.path,
      failure: situationFailure(portal.lighting),
      situation: portal.lighting,
      input: snapshot.input,
      device: snapshot.device,
    },
    leave: () => nav.back('settings'),
    preview: host.preview,
  };
  return (
    <Page
      className="portal-page section-page"
      title={section === 'capture' ? 'Prysm video source' : 'Prysm lights'}
      name={copy.title}
    >
      <PageHeading title={copy.title} to="settings" backLabel="Settings" />
      <OfflineNotice />
      <DiscoveryContext value={host.discovery ?? null}>
        <CaptureModesContext value={host.captureModes}>
          {section === 'capture' ? (
            <SectionEditor
              section="capture"
              adapters={host.captures}
              saved={settings.capture ?? null}
              previous={portal.previous.capture}
              setPrevious={(capture) =>
                portal.setPrevious({ ...portal.previous, capture })
              }
              save={(expected, capture) =>
                dispatch(commands.configureCapture(expected, capture))
              }
              {...shared}
            />
          ) : (
            <SectionEditor
              section="output"
              adapters={host.outputs}
              saved={settings.output ?? null}
              previous={portal.previous.output}
              setPrevious={(output) =>
                portal.setPrevious({ ...portal.previous, output })
              }
              save={(expected, output) =>
                dispatch(commands.configureOutput(expected, output))
              }
              {...shared}
            />
          )}
        </CaptureModesContext>
      </DiscoveryContext>
    </Page>
  );
}

export function CalibrationsRoute() {
  const portal = usePortal();
  const nav = useNav();
  return (
    <LivePage
      className="calibrations-page"
      title="Prysm calibrations"
      name="Calibrations"
      heading="Calibrations"
    >
      <CalibrationList
        {...useLiveProps()}
        deleted={portal.deletedCalibrations}
        setDeleted={portal.setDeletedCalibrations}
        open={(id) => nav.go({ calibration: id })}
        calibrate={() => nav.go('calibrate')}
      />
    </LivePage>
  );
}

export function CalibrationRoute() {
  const portal = usePortal();
  const nav = useNav();
  const live = useLiveProps();
  const id = useParams().id ?? '';
  const calibration = live.processing.calibrations?.find(
    (item) => item.id === id,
  );
  const heading = calibration ? calibration.name : 'Calibration';
  return (
    <LivePage
      className="calibration-page"
      title="Prysm calibration"
      name={heading}
      heading={heading}
      to="calibrations"
      backLabel="Calibrations"
    >
      <CalibrationPage
        {...live}
        id={id}
        deleted={(gone) =>
          portal.setDeletedCalibrations((current) => ({
            ...current,
            [id]: gone,
          }))
        }
        leave={() => nav.back('calibrations')}
      />
    </LivePage>
  );
}

export function StyleRoute() {
  return (
    <LivePage
      className="style-page"
      title="Prysm style"
      name="Style"
      heading="Style"
    >
      <StyleSettings {...useLiveProps()} />
    </LivePage>
  );
}

export function DetailRoute() {
  return (
    <LivePage
      className="detail-page"
      title="Prysm detail"
      name="Detail"
      heading="Detail"
    >
      <Detail {...useLiveProps()} />
    </LivePage>
  );
}

export function AppearanceRoute() {
  return (
    <LivePage
      className="appearance-page"
      title="Prysm appearance"
      name="Appearance"
      heading="Appearance"
      note="Kept in this browser"
    >
      <ThemeChoice />
    </LivePage>
  );
}

export function AboutRoute() {
  const { snapshot } = usePortal();
  return (
    <LivePage
      className="about-page"
      title="About this Prysm"
      name="About this Prysm"
      heading="About this Prysm"
      note=""
    >
      <AboutPage snapshot={snapshot} />
    </LivePage>
  );
}

export function CalibrateRoute() {
  const {
    snapshot,
    online,
    dispatch,
    commands,
    saveProcessing,
    tryProcessing,
  } = usePortal();
  const nav = useNav();
  return (
    <Page {...calibrateFrame}>
      <OfflineNotice />
      <CalibrateWizard
        processing={snapshot.settings.processing}
        revision={snapshot.revision}
        target={snapshot.target}
        shown={snapshot.calibration}
        disabled={!online}
        save={saveProcessing}
        tryProcessing={tryProcessing}
        setCalibration={(pattern) => dispatch(commands.setCalibration(pattern))}
        // Back where it was opened (Home, or the calibrations), else Home.
        leave={nav.backToOpener}
      />
    </Page>
  );
}

/**
 * Home: the light card (power, mode, brightness, style, calibration) and, once
 * set up, the path the light takes with the live preview. `gridRef` takes the
 * live LED colours.
 */
function HomePage<C extends AdapterSection, O extends AdapterSection>({
  snapshot,
  captures,
  outputs,
  discovery,
  preview,
  gridRef,
  online,
  lighting,
  path,
  setTarget,
  save,
  tryProcessing,
  openSetup,
  calibrate,
  go,
}: {
  snapshot: ApplicationSnapshot<C, O>;
  captures: AdapterUis<C>;
  outputs: AdapterUis<O>;
  discovery: DiscoveryClient | null;
  /** Whether this host streams the LEDs' colours at all. */
  preview?: PreviewSource;
  gridRef: RefObject<HTMLDivElement | null>;
  online: boolean;
  lighting: LightingSituation;
  path: PathState;
  setTarget: (target: LightingTarget) => Promise<unknown>;
  save: (expected: number, processing: Processing) => Promise<number>;
  tryProcessing: (processing: Processing) => Promise<unknown>;
  openSetup: () => void;
  calibrate: () => void;
  go: (place: Place) => void;
}) {
  const { settings, revision } = snapshot;
  const capture = settings.capture ?? null;
  const output = settings.output ?? null;
  const captureUi = capture ? adapterFor(captures, capture.kind) : undefined;
  const outputUi = output ? adapterFor(outputs, output.kind) : undefined;
  const edges = output ? outputUi?.edges?.(output) : undefined;
  return (
    <div className="home-grid" ref={gridRef}>
      <LightCard
        target={snapshot.target}
        lighting={lighting}
        device={snapshot.device}
        sentLedCount={edges ? ledCount(edges) : undefined}
        hasCapture={!!capture}
        hasOutput={!!output}
        output={output ? (outputUi?.lighting?.(output) ?? {}) : null}
        disabled={!online}
        setTarget={setTarget}
        openSetup={openSetup}
      >
        <QuickPicture
          processing={settings.processing}
          revision={revision}
          disabled={!online}
          save={save}
          adjustStyle={() => go('style')}
          calibrate={calibrate}
          tryProcessing={tryProcessing}
        />
      </LightCard>
      {/* Before setup the light card's prompt says it all. */}
      {capture && output ? (
        <SetupCard
          capture={{
            section: 'capture',
            saved: capture,
            label: captureUi?.label ?? capture.kind,
            facts: captureUi?.facts?.(capture) ?? [],
          }}
          output={{
            section: 'output',
            saved: output,
            label: outputUi?.label ?? output.kind,
            facts: outputUi?.facts?.(output) ?? [],
          }}
          edges={edges}
          transports={{
            capture: captureUi?.transport?.(capture),
            output: outputUi?.transport?.(output),
          }}
          discovery={discovery}
          path={path}
          calibration={activeCalibration(settings.processing)}
          preview={preview}
          openSettings={() => {
            if (path.kind === 'failed' && path.at === 'capture') go('video');
            else if (path.kind === 'failed' && path.at === 'output')
              go('lights');
            else go('settings');
          }}
        />
      ) : null}
    </div>
  );
}
