/** The lighting card: what the lights do on the Prysm that drives them. */
import { useState } from 'react';
import {
  describeLighting,
  describeOutputIssue,
  failureOf,
  plainFailure,
  type ApplicationSnapshot,
  type LightingTarget,
} from '@prysm/application-client';
import type { PortalView } from './portal-connection';

type Snapshot = ApplicationSnapshot<unknown, unknown>;

const outputKind = (snapshot: Snapshot) =>
  (snapshot.settings.output as { kind?: string } | undefined)?.kind;
const effectsOf = (snapshot: Snapshot) =>
  outputKind(snapshot) === 'wled' ? 'WLED effects' : null;

/**
 * The lighting card: the three modes, always in the same place, and one
 * message under them (at most two short lines) when the chosen mode does not
 * say it all: starting, a problem, or why the buttons cannot work.
 */
export function LightingCard({
  flavor,
  portal,
}: {
  flavor: 'full' | 'slim';
  portal: PortalView;
}) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const [error, setError] = useState('');
  const { snapshot, connection } = portal;
  const full = flavor === 'full';
  const effects = snapshot ? effectsOf(snapshot) : null;

  async function choose(target: LightingTarget) {
    setPending(target);
    setError('');
    try {
      await portal.setTarget(target);
    } catch (reason) {
      setError(
        plainFailure(failureOf(reason), target, effects ?? 'Device effects'),
      );
    } finally {
      setPending(null);
    }
  }

  /** Shown under the modes; `null` when the chosen mode says it all. */
  let message: string | null = null;
  let attention = false;
  /** For screen readers when nothing is shown: what the lights do. */
  let said = '';
  if (!portal.baseUrl) {
    message = full
      ? 'Waiting for Prysm on this TV…'
      : 'Add the Prysm address to control the lights here.';
  } else if (!portal.supported) {
    message =
      'This TV is too old to control the lights. Use the web portal on your phone.';
    attention = true;
  } else if (snapshot) {
    const { session, target } = snapshot;
    said = describeLighting({
      ...snapshot,
      effects: effects ?? 'Device effects',
    });
    const kind = (snapshot.settings.capture as { kind?: string } | undefined)
      ?.kind;
    if (session.state === 'faulted') {
      message = said;
      attention = true;
    } else if (!snapshot.settings.output) {
      message = 'Set up your lights in the web portal first.';
    } else if (snapshot.failure) {
      message = plainFailure(
        snapshot.failure,
        target,
        effects ?? 'Device effects',
      );
      attention = true;
    } else if (snapshot.output_issue && session.state === 'running') {
      message = describeOutputIssue(snapshot.output_issue);
      attention = true;
    } else if (!full && kind !== 'remote') {
      message =
        'This Prysm uses another picture. Choose Capture-only TV in its web portal.';
      attention = true;
    } else if (session.state === 'stopped' && target !== 'off') {
      message = said;
    } else if (
      session.state === 'running' &&
      snapshot.input &&
      !snapshot.input.receiving
    ) {
      message = 'No picture arriving yet.';
    }
  } else if (portal.reach === 'refused') {
    message = full
      ? 'Prysm on this TV doesn’t answer the app. Close and open the app again.'
      : 'Prysm on your computer doesn’t take commands from the TV. The TV guide shows how to allow it.';
    attention = true;
  } else if (connection?.error) {
    message = full
      ? 'Can’t reach Prysm on this TV. Service options can repair it.'
      : 'Can’t reach Prysm on your computer. Check that it’s running.';
    attention = true;
  } else message = 'Connecting to Prysm…';
  if (error) {
    message = error;
    attention = true;
  }

  const offline = !snapshot || pending !== null;
  const faulted = snapshot?.session.state === 'faulted';
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
      label: effects ?? 'Effects',
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
        Your lights
      </h2>
      <div
        className="modes"
        data-row="lighting"
        role="group"
        aria-label="Lighting mode"
      >
        {modes.map((mode) => (
          <button
            key={mode.value}
            type="button"
            id={`mode-${mode.value}`}
            className="mode"
            aria-pressed={snapshot ? snapshot.target === mode.value : undefined}
            disabled={offline || faulted || mode.disabled}
            onClick={() => void choose(mode.value)}
          >
            {mode.label}
          </button>
        ))}
      </div>
      {/* Two lines are always kept, so nothing moves when a message comes. */}
      <p
        id="lighting-status"
        className="lighting-status"
        role="status"
        aria-live="polite"
      >
        {message ?? <span className="visually-hidden">{said}</span>}
      </p>
    </section>
  );
}
