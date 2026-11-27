import {
  pictureOf,
  type AdapterSide,
  type ApplicationSnapshot,
} from '@prysm/application-client';
import { useNav, usePortal } from './portal-context';
import { ledCount, setupPart, unbroken, type SetupPart } from './adapters';
import { saveDeveloper, useDeveloper } from './developer';
import { useDeviceName } from './device-name';
import { CheckboxField } from './fields';
import { FieldScopeProvider, useFieldErrors, useFieldScope } from './form';
import type { ProcessingEditor } from './live-processing';
import { BlackBars } from './processing';
import { Group, RowFact, RowLink, RowSwitch } from './rows';
import { styleOf, styles } from './styles';
import { currentTheme, themeChoices } from './theme';

function SetupRow({
  part,
  label,
  problem,
  open,
}: {
  part: SetupPart;
  label: string;
  problem: string | null;
  open: () => void;
}) {
  const { facts, edges } = part;
  const name = useDeviceName(part);
  const details = edges
    ? [`${ledCount(edges)} LEDs`]
    : facts
        // As on Home: whether TV menus are included is not a fact to list.
        .filter((fact) => fact.label !== 'Menus')
        .map((fact) => fact.value);
  const shownName = unbroken(name);
  return (
    <RowLink
      label={label}
      value={[shownName, ...details].join(' · ')}
      lead={shownName}
      problem={problem}
      onClick={open}
    />
  );
}

export function SettingsPage({ live }: { live: ProcessingEditor }) {
  const {
    snapshot: { settings },
    host,
    problemAt,
    openSetup,
  } = usePortal();
  const { go } = useNav();
  const style = styleOf(settings.processing.style.kind);
  const processing = settings.processing;
  const calibrations = processing.calibrations ?? [];
  const active = calibrations.find(
    ({ id }) => id === processing.active_calibration,
  );
  let calibrationLine = 'None yet';
  if (calibrations.length)
    calibrationLine = `${calibrations.length} ${calibrations.length === 1 ? 'calibration' : 'calibrations'} · ${active ? `${active.name} in use` : 'none in use'}`;
  const theme = themeChoices.find(({ value }) => value === currentTheme());

  function setupRow(section: AdapterSide) {
    const saved = settings[section];
    const capture = section === 'capture';
    const label = capture ? 'Video source' : 'Lights';
    const open = () => go(capture ? 'video' : 'lights');
    if (!saved)
      return <RowLink label={label} value="Not set up yet" onClick={open} />;
    const panels = capture ? host.captures : host.outputs;
    return (
      <SetupRow
        label={label}
        part={setupPart(section, panels, saved)}
        problem={problemAt(section)}
        open={open}
      />
    );
  }
  return (
    <>
      <Group title="Setup">
        {setupRow('capture')}
        {setupRow('output')}
      </Group>
      <Group title="Picture">
        <RowLink
          label="Style"
          value={style ? styles[style].label : 'Unknown style'}
          onClick={() => go('style')}
        />
        <RowLink
          label="Calibrations"
          value={calibrationLine}
          onClick={() => go('calibrations')}
        />
        <RowLink
          label="Detail"
          value="How closely the lights follow small details"
          onClick={() => go('detail')}
        />
        <RowSwitch>
          <BlackBars live={live} />
        </RowSwitch>
      </Group>
      <Group title="This device">
        <RowLink
          label="Appearance"
          value={theme?.label ?? 'System'}
          onClick={() => go('appearance')}
        />
        <RowLink
          label="About this Prysm"
          value="Status, version and developer options"
          onClick={() => go('about')}
        />
        <RowLink
          label="Start over with setup"
          value="Choose the video source and lights again, step by step"
          onClick={openSetup}
        />
      </Group>
    </>
  );
}

const targetLabels = {
  off: 'Off',
  screen_sync: 'Screen sync',
  effects: 'The lights’ own effects',
  unknown: 'Unknown: Prysm is newer than this page',
} as const satisfies Record<
  ApplicationSnapshot<unknown, unknown>['target'],
  string
>;
const sessionLabels = {
  stopped: 'Stopped',
  screen_sync: 'Screen sync',
  effects: 'Showing the lights’ own effects',
  unanswered: 'Waiting for the lights to answer',
  faulted: 'Stopped by a fault; restart Prysm',
  unknown: 'Unknown: Prysm is newer than this page',
} as const satisfies Record<
  ApplicationSnapshot<unknown, unknown>['session']['state'],
  string
>;

export function AboutPage({
  snapshot,
}: {
  snapshot: ApplicationSnapshot<unknown, unknown>;
}) {
  const developer = useDeveloper();
  const errors = useFieldErrors();
  const scope = useFieldScope(false, errors, false);
  return (
    <>
      <Group title="Now">
        <RowFact label="Lights" value={targetLabels[snapshot.target]} />
        <RowFact label="Status" value={sessionLabels[snapshot.session.state]} />
        <RowFact
          label="Frames per second"
          value={
            snapshot.input && pictureOf(snapshot.input) === 'arriving'
              ? String(Math.round(snapshot.input.fps))
              : 'No picture'
          }
        />
        <RowFact label="Prysm version" value={snapshot.version} />
        <RowFact label="Settings revision" value={String(snapshot.revision)} />
      </Group>
      <Group title="This browser">
        <RowSwitch>
          <FieldScopeProvider value={scope}>
            <CheckboxField
              path="developer"
              label="Developer options"
              help="Also offers a video file and “No lights” as a setup, for testing."
              checked={developer}
              onChange={saveDeveloper}
            />
          </FieldScopeProvider>
        </RowSwitch>
      </Group>
    </>
  );
}
