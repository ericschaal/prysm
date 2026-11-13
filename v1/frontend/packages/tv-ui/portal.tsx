/** The lighting card: what the lights do on the Prysm that drives them. */
import { useState } from 'react';
import {
  describeLighting,
  describeOutputIssue,
  failureOf,
  type ApplicationSnapshot,
  type LightingTarget,
} from '@prysm/application-client';
import type { PortalView } from './portal-connection';

type Snapshot = ApplicationSnapshot<unknown, unknown>;

const outputKind = (snapshot: Snapshot) =>
  (snapshot.settings.output as { kind?: string } | undefined)?.kind;
const effectsOf = (snapshot: Snapshot) =>
  outputKind(snapshot) === 'wled' ? 'WLED effects' : null;

const captureNames: Record<string, string> = {
  remote: 'a TV (Remote capture)',
  device: 'a video device',
  file: 'a video file',
  webos: 'this TV',
};

/** The lighting card: what the lights do and the three modes. */
export function LightingCard({
  flavor,
  portal,
  address,
}: {
  flavor: 'full' | 'slim';
  portal: PortalView;
  /** The portal's address for people on other devices (slim: as typed). */
  address: string | null;
}) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const [error, setError] = useState('');
  const { snapshot, connection } = portal;
  const where =
    flavor === 'full' ? 'the Prysm on this TV' : `Prysm at ${address ?? ''}`;

  async function choose(target: LightingTarget) {
    setPending(target);
    setError('');
    try {
      await portal.setTarget(target);
    } catch (reason) {
      setError(failureOf(reason).message);
    } finally {
      setPending(null);
    }
  }

  let status: string;
  let detail: string | null = null;
  let attention = false;
  /**
   * The status only repeats the chosen mode and the detail only repeats the
   * capture rate: kept for screen readers, not shown.
   */
  let quietStatus = false;
  let quietDetail = false;
  if (!portal.baseUrl) {
    status =
      flavor === 'full'
        ? 'Waiting for the web portal on this TV…'
        : 'Add the Prysm address to control its lights from here.';
  } else if (!portal.supported) {
    status = 'This TV’s browser is too old to control Prysm from the app.';
    detail = `Open ${address ?? portal.baseUrl} on your phone or computer instead.`;
    attention = true;
  } else if (snapshot) {
    status = describeLighting({
      ...snapshot,
      effects: effectsOf(snapshot) ?? 'Light effects',
    });
    const { state } = snapshot.session;
    quietStatus =
      state === 'running' ||
      state === 'native' ||
      (state === 'stopped' && snapshot.target === 'off');
    if (snapshot.input && state === 'running') {
      detail = snapshot.input.receiving
        ? `Picture arriving at ${snapshot.input.fps.toFixed(1)} frames per second.`
        : 'No picture arriving yet.';
      quietDetail = snapshot.input.receiving;
    }
    if (snapshot.output_issue && snapshot.session.state === 'running') {
      detail = describeOutputIssue(snapshot.output_issue);
      attention = true;
      quietDetail = false;
    }
    if (flavor === 'slim') {
      const kind = (snapshot.settings.capture as { kind?: string } | undefined)
        ?.kind;
      if (kind !== 'remote') {
        detail = `This Prysm takes its picture from ${kind ? (captureNames[kind] ?? kind) : 'nothing yet'}. Choose Remote capture in its portal to use this TV.`;
        attention = true;
        quietDetail = false;
      }
    }
  } else if (portal.reach === 'refused') {
    status = `${capitalize(where)} does not allow control from the TV.`;
    detail =
      flavor === 'slim'
        ? 'Start Prysm on your computer with --allow-origin null to use these buttons. Capture works without it.'
        : 'Restart the TV app; if this persists, check the Prysm service.';
    attention = true;
  } else if (connection?.error) {
    status = `Cannot reach ${where}.`;
    detail =
      portal.reach === 'unreachable'
        ? 'Check that Prysm is running and that the address is right.'
        : connection.error.message;
    attention = true;
  } else status = `Connecting to ${where}…`;

  const offline = !snapshot || pending !== null;
  const faulted = snapshot?.session.state === 'faulted';
  const effects = snapshot ? effectsOf(snapshot) : null;
  const hasOutput = !!snapshot?.settings.output;
  const hasCapture = !!snapshot?.settings.capture;
  const modes: { value: LightingTarget; label: string; disabled: boolean }[] = [
    {
      value: 'processed',
      label: 'Screen sync',
      disabled: !hasOutput || !hasCapture,
    },
    {
      value: 'device_native',
      label: effects ?? 'Light effects',
      disabled: !effects,
    },
    { value: 'off', label: 'Off', disabled: false },
  ];
  return (
    <section
      className={`card lighting${attention ? ' attention' : ''}`}
      aria-labelledby="lighting-title"
    >
      <h2 id="lighting-title" className="card-title">
        Lights show
      </h2>
      <p
        id="lighting-status"
        className={`lighting-status${quietStatus ? ' visually-hidden' : ''}`}
        role="status"
        aria-live="polite"
      >
        {status}
      </p>
      <div
        className="modes"
        data-row="lighting"
        role="group"
        aria-label="Lighting mode"
        hidden={!snapshot}
      >
        {modes.map((mode) => (
          <button
            key={mode.value}
            type="button"
            id={`mode-${mode.value}`}
            className="mode"
            aria-pressed={snapshot?.target === mode.value}
            disabled={offline || faulted || mode.disabled}
            onClick={() => void choose(mode.value)}
          >
            {mode.label}
          </button>
        ))}
      </div>
      {detail ? (
        <p
          id="lighting-detail"
          className={`lighting-detail${quietDetail ? ' visually-hidden' : ''}`}
        >
          {detail}
        </p>
      ) : null}
      {snapshot && !hasOutput ? (
        <p className="lighting-detail">
          Set up your lights in the web portal first.
        </p>
      ) : null}
      {snapshot?.failure ? (
        <p className="notice error" role="alert">
          {snapshot.failure.message}
        </p>
      ) : null}
      {error && error !== snapshot?.failure?.message ? (
        <p className="notice error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}

const capitalize = (text: string) =>
  text.charAt(0).toUpperCase() + text.slice(1);
