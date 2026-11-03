import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { useControl } from './useControl';
import { Overview } from './Overview';
import { Settings } from './Settings';
import { Preview } from './Preview';
import './style.css';

const currentPage = () =>
  window.location.hash === '#settings'
    ? 'settings'
    : window.location.hash === '#preview'
      ? 'preview'
      : 'overview';

export function App() {
  const control = useControl();
  const [page, setPage] = useState(currentPage);
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    const navigate = () => setPage(currentPage());
    window.addEventListener('hashchange', navigate);
    return () => window.removeEventListener('hashchange', navigate);
  }, []);
  const online = !!control.settings && !control.connectionError;
  const setup = !!control.settings?.missing_required.length;
  const title = setup
    ? 'Bring your screen to life.'
    : page === 'overview'
      ? 'Light, in sync.'
      : page === 'preview'
        ? 'See what your lights see.'
        : 'Make Prysm yours.';
  return (
    <div className={`app-shell${setup ? ' is-setup' : ''}`}>
      <a className="skip-link" href="#content">
        Skip to content
      </a>
      <header className="app-header">
        <a className="brand" href="#overview" aria-label="Prysm home">
          <svg viewBox="0 0 32 32" fill="none" aria-hidden="true">
            <path
              d="m16 3 13 24H3L16 3Z"
              stroke="currentColor"
              strokeWidth="2"
            />
            <path
              d="m16 3 4 24M3 27l15-12 11 12"
              stroke="currentColor"
              strokeWidth="1.5"
            />
          </svg>
          <span>
            prysm<span className="brand-dot">.</span>
          </span>
        </a>
        <span
          className={`connection ${online ? 'ready' : 'attention'}`}
          role="status"
        >
          <i />
          {control.connectionError
            ? 'Offline'
            : online
              ? 'Connected'
              : 'Connecting'}
        </span>
      </header>
      {!setup ? (
        <nav className="main-nav" aria-label="Main navigation">
          <a
            href="#overview"
            aria-current={page === 'overview' ? 'page' : undefined}
          >
            Overview
          </a>
          <a
            href="#preview"
            aria-current={page === 'preview' ? 'page' : undefined}
          >
            Preview
          </a>
          <a
            href="#settings"
            aria-current={page === 'settings' ? 'page' : undefined}
          >
            Settings{' '}
            {dirty ? (
              <span className="draft-dot" aria-label="unsaved changes" />
            ) : null}
          </a>
        </nav>
      ) : null}
      <main id="content">
        <div className="page-intro">
          <p className="eyebrow">
            {setup
              ? 'YOUR FIRST SCREEN SYNC'
              : page === 'overview'
                ? 'ROOM CONTROL'
                : page === 'preview'
                  ? 'PICTURE & COLOR'
                  : 'SCREEN, LIGHTS & CONNECTIONS'}
          </p>
          <h1>{title}</h1>
          <p>
            {setup
              ? 'Choose your video source and tell Prysm about your lights.'
              : page === 'overview'
                ? 'Control your lights and see what Prysm is doing.'
                : page === 'preview'
                  ? 'Inspect the picture, detected crop, and colors sent to each edge.'
                  : 'Configure everything in one place, then save to apply your changes.'}
          </p>
        </div>
        {control.settings ? (
          <>
            {!setup ? (
              <>
                <div hidden={page !== 'overview'}>
                  <Overview {...control} />
                </div>
                <div hidden={page !== 'preview'} className="preview-page">
                  <Preview active={page === 'preview'} data={control.data} />
                  {control.data?.operation.kind === 'failed' ? (
                    <p className="notice error" role="alert">
                      Application failed: {control.data.operation.error.message}
                      .
                    </p>
                  ) : null}
                  <p>
                    Change capture, LED layout, and picture processing in{' '}
                    <a href="#settings">Settings</a>.
                  </p>
                </div>
              </>
            ) : null}
            <div hidden={!setup && page !== 'settings'}>
              <Settings
                settings={control.settings}
                data={control.data}
                disabled={!online || control.busy}
                save={control.saveSettings}
                onDirty={setDirty}
              />
            </div>
            {control.connectionError ? (
              <p className="notice error" role="alert">
                Controller connection lost. Your draft is preserved.{' '}
                {control.connectionError}
              </p>
            ) : null}
          </>
        ) : (
          <section className="card">
            <h2>Loading Prysm</h2>
            <p>{control.connectionError || 'Waiting for settings…'}</p>
            <button type="button" onClick={() => void control.poll()}>
              Check connection
            </button>
          </section>
        )}
      </main>
    </div>
  );
}
createRoot(document.getElementById('root')!).render(<App />);
