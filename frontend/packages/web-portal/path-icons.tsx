/** The three stops of the signal path, drawn the same on Home and in Settings. */

export function VideoIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <rect x="3" y="4.5" width="18" height="12" rx="2" />
      <path d="M8.5 20h7M12 16.5V20" />
    </svg>
  );
}

/** Prysm's mark: the prism the light passes through. */
export function PrysmIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M12 3.5 21 19.5H3Z" />
      <path className="ray" d="M12 3.5l2.6 16M3 19.5 13.6 11l7.4 8.5" />
    </svg>
  );
}

export function LightsIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M9.5 18h5M10.5 21h3M8 14.5a6 6 0 1 1 8 0c-.9.8-1.3 1.7-1.3 2.5H9.3c0-.8-.4-1.7-1.3-2.5Z" />
    </svg>
  );
}
