import { useEffect, useRef, type ReactNode } from 'react';
import { DeveloperContext } from './developer';
import { useChrome, usePortal, usePortalRefs } from './portal-context';
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

function Header() {
  const chrome = useChrome();
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
        {chrome.settingsLink ? (
          <button
            type="button"
            className="header-link"
            onClick={chrome.settingsLink}
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
  children,
}: {
  className: string;
  title: string;
  name?: string;
  wide?: boolean;
  children: ReactNode;
}) {
  const { developer, updated } = useChrome();
  return (
    <div className={wide ? 'portal wide' : 'portal'}>
      <title>{name ? `${name} · Prysm` : 'Prysm'}</title>
      <Header />
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
  to: string;
  backLabel: string;
}) {
  const { nav } = usePortal();
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
  const { setupNotice } = usePortal();
  const { notice } = usePortalRefs();
  return setupNotice ? (
    <p className="notice" role="status" tabIndex={-1} ref={notice}>
      {setupNotice}
    </p>
  ) : null;
}
