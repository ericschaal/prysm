/**
 * Calibration on the TV: the phone (the portal's `#/calibrate` page) chooses
 * patterns; the TV shows the one in `State.calibration` as full-screen video,
 * and a QR code of that page while none is chosen.
 */
import { useEffect, useMemo, useRef, useState, type Ref } from 'react';
import type { Pattern } from '@prysm/application-client';
import type { State } from '@prysm/webos-platform';
import type { PortalView } from './portal';
import { WAITING_PATTERN, type PatternPlayer } from './patterns';
import { qrCode } from './qr';

/** The portal page the phone opens, from the portal address the TV shows. */
export const calibrationUrl = (portal: string) => `${portal.replace(/\/+$/, '')}/#/calibrate`;

/**
 * Why the lights cannot be calibrated from this TV's picture, or `null`.
 * `tv` is the supervisor's state; `instance` the slim package's Prysm address.
 */
export function calibrationBlocker(
  flavor: 'full' | 'slim',
  player: PatternPlayer,
  portal: PortalView,
  tv: State | null,
  instance: string | null,
): string | null {
  if (!player.supported || !portal.supported) return 'Calibration needs a newer TV';
  if (flavor === 'slim' && !instance) return 'Add the Prysm address first';
  if (flavor === 'slim' && !tv?.enabled) return 'Start capture first';
  const { snapshot } = portal;
  if (!snapshot) return 'Needs a connection to Prysm';
  if (!snapshot.settings.capture || !snapshot.settings.output)
    return 'Set up capture and lights first';
  const kind = (snapshot.settings.capture as { kind?: string }).kind;
  if (flavor === 'slim' && kind !== 'remote') return 'Prysm is not using this TV’s picture';
  return null;
}

function QrCode({ modules }: { modules: boolean[][] }) {
  // A four-module quiet zone around the code.
  const size = modules.length + 8;
  let path = '';
  modules.forEach((row, y) =>
    row.forEach((dark, x) => {
      if (dark) path += `M${x + 4} ${y + 4}h1v1h-1z`;
    }),
  );
  return (
    <svg
      id="calibration-qr"
      className="qr"
      viewBox={`0 0 ${size} ${size}`}
      shapeRendering="crispEdges"
      role="img"
      aria-label="QR code of the address below"
    >
      <rect width={size} height={size} fill="#fff" />
      <path d={path} fill="#000" />
    </svg>
  );
}

/** The pattern video; the stream keeps running while `pattern` changes. */
function PatternVideo({
  player,
  pattern,
  failed,
}: {
  player: PatternPlayer;
  pattern: Pattern;
  failed: () => void;
}) {
  const video = useRef<HTMLVideoElement>(null);
  const latest = useRef({ pattern, failed });
  latest.current = { pattern, failed };
  useEffect(
    () =>
      player.play(
        video.current!,
        () => latest.current.pattern,
        () => latest.current.failed(),
      ),
    [player],
  );
  return <video ref={video} id="pattern-video" className="pattern-video" muted playsInline />;
}

/**
 * Full screen: the pattern video, under a QR code while the phone has not
 * chosen a pattern (the gray pattern plays meanwhile, so the TV's video
 * pipeline and the capture are already running when the first one comes).
 */
export function CalibrationScreen({
  root,
  player,
  pattern,
  address,
}: {
  root: Ref<HTMLElement>;
  player: PatternPlayer;
  /** `State.calibration.pattern`, absent while none is chosen. */
  pattern: Pattern | undefined;
  /** The portal address the TV shows. */
  address: string;
}) {
  const [failed, setFailed] = useState(false);
  const url = calibrationUrl(address);
  const modules = useMemo(() => qrCode(url), [url]);
  return (
    <section ref={root} className={`calibration${pattern && !failed ? ' showing' : ''}`} aria-label="Calibration">
      {failed ? null : (
        <PatternVideo player={player} pattern={pattern ?? WAITING_PATTERN} failed={() => setFailed(true)} />
      )}
      {pattern && !failed ? null : (
        <div className="calibration-waiting">
          <h1>Calibrate colours</h1>
          {modules ? <QrCode modules={modules} /> : null}
          <code id="calibration-address">{url}</code>
          <p id="calibration-hint">
            {modules ? 'Scan with your phone to start.' : 'Open this address on your phone to start.'}
          </p>
          {failed ? (
            <p id="calibration-error" className="notice error" role="alert">
              The pattern video stopped. Press Back and try again.
            </p>
          ) : null}
          <footer className="key-help">
            <span>
              <b>Back</b> Return
            </span>
          </footer>
        </div>
      )}
    </section>
  );
}
