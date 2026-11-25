/** The Diagnostics text: the one place for technical words. */
import { REFUSED_VIDEO } from '@prysm/application-client';
import {
  fullPortal,
  isCapturing,
  isEnabled,
  isPaused,
  isRefused,
  problemOf,
  slimEncoding,
  type State,
} from '@prysm/webos-platform';
import { captureRate, encodingName } from './screen';

const powerNames = {
  unknown: 'unknown',
  active: 'on',
  screen_saver: 'screen saver',
  asleep: 'standby',
} as const;
const phaseNames = {
  stopped: 'stopped',
  running: 'running',
  stopping: 'stopping',
  asking: 'stopped until you choose what to do',
  unknown: 'unknown (newer than this app)',
} as const;

function capturingText(state: State) {
  if (isCapturing(state)) return 'yes';
  if (isPaused(state))
    return 'no, paused by Prysm (lights off on the computer)';
  if (isRefused(state)) return `no. ${REFUSED_VIDEO}`;
  return 'no';
}

/**
 * The Diagnostics text: the one place for technical words. `problem` is the
 * last command's error as the service gave it.
 */
export function diagnostics(state: State | null, problem = ''): string {
  if (!state)
    return problem ? `Service unavailable: ${problem}` : 'Service unavailable.';
  const lines = [
    `Package: ${state.flavor === 'full' ? 'Prysm (lights from this TV)' : 'Prysm capture (lights from a computer)'}`,
    `TV power: ${powerNames[state.power]}`,
    `Video: ${state.video}`,
    `Enabled: ${isEnabled(state) ? 'yes' : 'no'}`,
    `Background process: ${phaseNames[state.worker.phase]}`,
    `Capturing: ${capturingText(state)}`,
    `Capture rate: ${captureRate(state) ?? 'not measured'}`,
  ];
  if (state.flavor === 'slim') {
    const encoding = slimEncoding(state);
    lines.push(
      `Prysm address: ${state.preferences.prysm_address ?? 'not set'}`,
      `Stream encoding: ${encoding ? `${encodingName(encoding)} in use` : 'none in use'}`,
    );
  } else lines.push(`Web portal: ${fullPortal(state) ?? 'not open'}`);
  const asking = problemOf(state);
  if (asking) {
    const { asked, message } = asking;
    const step = asked.step === 'ask' ? asked.fix : asked.step;
    lines.push('', `${message} (asked: ${step})`);
  } else lines.push('', 'No errors reported.');
  if (state.unreadable !== undefined)
    lines.push('', `Saved settings unreadable: ${state.unreadable}`);
  if (problem) lines.push('', `Last command failed: ${problem}`);
  if (state.log.length) lines.push('', 'Recent messages:', ...state.log);
  return lines.join('\n');
}
