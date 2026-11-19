import { useEffect, useRef, type ReactNode } from 'react';
import { DeveloperContext } from './developer';
import { useChrome, useNav, usePortal } from './portal-context';
import type { Place } from './routes';
import { UpdatedNotice } from './update';

function SettingsIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
      <circle cx="15" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </svg>
  );
}

function Header({ settingsLink }: { settingsLink: boolean }) {
  const chrome = useChrome();
  const nav = useNav();
  return (
    <header className="portal-header">
      <span className="brand">
        <svg
          className="brand-mark"
          viewBox="0 0 32 32"
          aria-hidden="true"
          focusable="false"
        >
          <path className="brand-prism" d="M16 4 28.5 26.5h-25Z" />
          <path
            className="brand-ray"
            d="M16 4l3.6 22.5M3.5 26.5 18.2 15 28.5 26.5"
          />
        </svg>
        prysm<span aria-hidden="true">.</span>
      </span>
      <span className="header-end">
        <span
          className={`connection ${chrome.outOfTouch ? 'offline' : 'online'}`}
          role="status"
        >
          {chrome.connectionLabel}
        </span>
        {settingsLink ? (
          <button
            type="button"
            className="header-link"
            onClick={() => nav.go('settings')}
          >
            <SettingsIcon />
            Settings
          </button>
        ) : null}
      </span>
    </header>
  );
}

/**
 * One page: the header, then the page's content under its (hidden) title.
 * `name` titles the browser tab: "Settings · Prysm"; without one, "Prysm".
 */
export function Page({
  className,
  title,
  name,
  wide = false,
  settingsLink = false,
  children,
}: {
  className: string;
  title: string;
  name?: string;
  wide?: boolean;
  /** A Settings button in the header: Home's way to Settings. */
  settingsLink?: boolean;
  children: ReactNode;
}) {
  const { developer, updated } = useChrome();
  return (
    <div className={wide ? 'portal wide' : 'portal'}>
      <title>{name ? `${name} · Prysm` : 'Prysm'}</title>
      <Header settingsLink={settingsLink} />
      <main id="content" className={className}>
        <h1 className="visually-hidden">{title}</h1>
        {updated ? <UpdatedNotice /> : null}
        <DeveloperContext value={developer}>{children}</DeveloperContext>
      </main>
    </div>
  );
}

/**
 * A page under Settings: its title and the way back. Arriving, the title
 * takes focus, for screen readers and keyboards.
 */
export function PageHeading({
  title,
  to,
  backLabel,
}: {
  title: string;
  to: Place;
  backLabel: string;
}) {
  const nav = useNav();
  const headingRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => headingRef.current?.focus({ preventScroll: true }), []);
  return (
    <div className="page-heading">
      <button type="button" className="back quiet" onClick={() => nav.back(to)}>
        <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
          <path d="M15 5.5 8.5 12l6.5 6.5" />
        </svg>
        {backLabel}
      </button>
      <h2 ref={headingRef} tabIndex={-1}>
        {title}
      </h2>
    </div>
  );
}

/** How setup ended, where it is shown (Home, Settings); focused by the frame. */
export function SetupNotice() {
  const { setupNotice, noticeRef } = usePortal();
  return setupNotice ? (
    <p className="notice" role="status" tabIndex={-1} ref={noticeRef}>
      {setupNotice}
    </p>
  ) : null;
}

/**
 * Says the connection is lost, once that lasts; the controls are disabled
 * at once, so a short gap (a restart) shows nothing.
 */
export function OfflineNotice() {
  const { outOfTouch } = useChrome();
  return outOfTouch ? (
    <p className="notice warning" role="alert">
      Lost the connection to Prysm. Reconnecting; your changes are kept.
    </p>
  ) : null;
}
