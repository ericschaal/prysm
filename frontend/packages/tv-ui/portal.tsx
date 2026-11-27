/** The lighting card: what the lights do on the Prysm that drives them. */
import { useState } from 'react';
import {
  describeOutputIssue,
  describeSituation,
  failureOf,
  lightingSituation,
  pictureOf,
  plainFailure,
  REFUSED_VIDEO,
  situationFailure,
  type LightingSituation,
  type LightingTarget,
  type Picture,
  type TargetReading,
} from '@prysm/application-client';
import type { Flavor } from '@prysm/webos-platform';
import type { ModeRoute } from './modes';
import type { PortalSnapshot, PortalView } from './portal-connection';

const effectsOf = (snapshot: PortalSnapshot) =>
  snapshot.settings.output?.kind === 'wled' ? 'WLED effects' : null;

/** The message while screen sync runs without a picture; not a problem. */
const waitingMessage: Record<Exclude<Picture, 'arriving'>, string> = {
  quiet: 'No picture arriving yet.',
  refused: REFUSED_VIDEO,
};

/**
 * The lighting card shows Power, the available modes while On, and one
 * message under them (at most two short lines) when the chosen mode does not
 * say it all: starting, a problem, or why the buttons cannot work.
 */
export function LightingCard({
  flavor,
  portal,
  connecting,
  route,
  choose: chooseOnService,
}: {
  flavor: Flavor;
  portal: PortalView;
  /** The TV service has not answered yet, so where Prysm is is not known. */
  connecting: boolean;
  /** Where a mode is sent. */
  route: ModeRoute;
  /** A mode sent to the TV service; it says its own failure. */
  choose: (target: LightingTarget | 'remembered') => Promise<unknown>;
}) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const { snapshot, connection } = portal;
  const full = flavor === 'full';
  const effects = snapshot ? effectsOf(snapshot) : null;
  const lighting = snapshot ? lightingSituation(snapshot) : null;
  /**
   * The latest refused press, with the lights as they were then: a command
   * that timed out may still have run, so it is shown only until they change.
   */
  const [refusal, setRefusal] = useState<{
    text: string;
    target: TargetReading | undefined;
    kind: LightingSituation['kind'] | undefined;
  } | null>(null);
  if (
    refusal &&
    (refusal.target !== snapshot?.target || refusal.kind !== lighting?.kind)
  )
    setRefusal(null);
  const error = refusal?.text ?? '';
  const onService = route.via === 'service';

  async function choose(target: LightingTarget, power = false) {
    const at = { target: snapshot?.target, kind: lighting?.kind };
    setPending(target);
    setRefusal(null);
    try {
      await (onService
        ? chooseOnService(power && !on ? 'remembered' : target)
        : portal.setTarget(target));
    } catch (reason) {
      setRefusal({
        text: plainFailure(
          failureOf(reason),
          target,
          effects ?? 'Device effects',
        ),
        ...at,
      });
    } finally {
      setPending(null);
    }
  }

  /** Shown under the modes; `null` when the chosen mode says it all. */
  let message: string | null = null;
  let attention = false;
  /** For screen readers when nothing is shown: what the lights do. */
  let said = '';
  if (connecting) {
    message = 'Connecting to Prysm…';
  } else if (onService) {
    said = 'Prysm stopped.';
  } else if (!portal.baseUrl) {
    message = full
      ? 'Waiting for Prysm on this TV…'
      : 'Add the Prysm address to control the lights here.';
  } else if (!portal.supported) {
    message =
      'This TV is too old to control the lights. Use the web portal on your phone.';
    attention = true;
  } else if (snapshot && lighting) {
    said = describeSituation(lighting, effects ?? 'Device effects').sentence;
    const failure = situationFailure(lighting);
    const kind = snapshot.settings.capture?.kind;
    if (lighting.kind === 'faulted') {
      message = said;
      attention = true;
    } else if (lighting.kind === 'unknown') {
      const carried = failure
        ? ` ${plainFailure(failure, 'unknown', effects ?? 'Device effects')}`
        : '';
      message = `Prysm is newer than this app. Update the app to see what the lights do.${carried}`;
      attention = !!failure;
    } else if (!snapshot.settings.output) {
      message = 'Set up your lights in the web portal first.';
    } else if (snapshot.control?.waiting) {
      message =
        snapshot.control.waiting === 'capture'
          ? 'Waiting for the video source.'
          : 'Waiting for the lights.';
    } else if (failure) {
      const still =
        lighting.kind === 'failed' && lighting.effectsOn
          ? ` ${effects ?? 'Device effects'} are still on.`
          : '';
      message = `${plainFailure(
        failure,
        snapshot.target,
        effects ?? 'Device effects',
      )}${still}`;
      attention = true;
    } else if (snapshot.control?.external) {
      message = 'Live stream detected.';
    } else if (snapshot.device.issue && lighting.kind === 'sync') {
      message = describeOutputIssue(snapshot.device.issue);
      attention = true;
    } else if (lighting.kind === 'unanswered') {
      message = said;
      attention = true;
    } else if (!full && kind !== 'remote') {
      message =
        'This Prysm uses another picture. Choose LG TV on your network in its web portal.';
      attention = true;
    } else if (lighting.kind === 'starting') {
      message = said;
    } else if (lighting.kind === 'sync' && lighting.input) {
      const picture = pictureOf(lighting.input);
      if (picture !== 'arriving') message = waitingMessage[picture];
    }
  } else if (portal.reach === 'refused') {
    message = full
      ? 'Prysm on this TV doesn’t answer the app. Close and open the app again.'
      : 'Prysm on your computer doesn’t take commands from the TV. The TV guide shows how to allow it.';
    attention = true;
  } else if (portal.stopped) {
    message = portal.stopped;
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

  // A press in flight does not disable the track: a disabled control drops
  // focus, which the remote navigation then moves to another column. On
  // the service Power resumes the mode stored by the worker.
  const offline = !snapshot && !onService;
  const faulted = lighting?.kind === 'faulted';
  const hasOutput = !!snapshot?.settings.output;
  const hasCapture = !!snapshot?.settings.capture;
  const chosen = snapshot?.control?.external ? undefined : snapshot?.target;
  const on = !onService && chosen !== 'off';
  const resume =
    snapshot?.control?.remembered ?? (effects ? 'effects' : 'screen_sync');
  const choices: { value: LightingTarget; label: string; disabled: boolean }[] =
    [
      {
        value: 'screen_sync',
        label: 'Screen sync',
        disabled: !onService && (!hasOutput || !hasCapture),
      },
      {
        value: 'effects',
        label: effects ?? 'Effects',
        disabled: !onService && !effects,
      },
    ];
  const showModes = on && !!effects;
  const modes = [
    {
      value: on ? ('off' as const) : resume,
      label: 'Power',
      disabled:
        !on &&
        !onService &&
        (!hasOutput || (resume === 'screen_sync' && !hasCapture)),
    },
    ...(showModes ? choices : []),
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
            key={mode.label}
            type="button"
            id={mode.label === 'Power' ? 'light-power' : `mode-${mode.value}`}
            className="mode"
            aria-pressed={
              mode.label === 'Power' ? !offline && on : chosen === mode.value
            }
            disabled={offline || faulted || mode.disabled}
            aria-busy={pending === mode.value ? true : undefined}
            onClick={() => {
              if (
                pending === null &&
                !(mode.label === 'Power' && snapshot?.control?.busy)
              )
                void choose(mode.value, mode.label === 'Power');
            }}
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
