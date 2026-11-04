import { useState } from 'react';
import {
  HashRouter,
  Link,
  NavLink,
  Navigate,
  Route,
  Routes,
  useParams,
} from 'react-router';
import { createRoot } from 'react-dom/client';
import { useControl } from './useControl';
import { Overview } from './Overview';
import { Settings } from './Settings';
import { Preview } from './Preview';
import './style.css';
import { Calibration } from './Calibration';

function CalibrationPage() {
  const { pairing = '' } = useParams();
  const control = useControl();
  return (
    <main id="content" className="calibration-page">
      <header className="calibration-brand">
        <span>
          prysm<span className="brand-dot">.</span>
        </span>
        <span>LIGHT & COLOR</span>
      </header>
      {control.settings ? (
        <Calibration
          key={pairing}
          pairing={pairing}
          settings={control.settings}
        />
      ) : (
        <section
          className="calibration-wizard calibration-loading"
          aria-busy={!control.connectionError}
        >
          <p className="eyebrow">MATCH YOUR LIGHTS</p>
          <h1>Connecting to your TV…</h1>
          <p role="status">
            {control.connectionError || 'Getting your profiles ready.'}
          </p>
          {control.connectionError ? (
            <button onClick={() => void control.poll()}>Try again</button>
          ) : null}
        </section>
      )}
    </main>
  );
}

function Dashboard({ page }: { page: 'overview' | 'preview' | 'settings' }) {
  const control = useControl();
  const [dirty, setDirty] = useState(false);
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
      <a
        className="skip-link"
        href="#content"
        onClick={(event) => {
          event.preventDefault();
          document.getElementById('content')?.focus();
        }}
      >
        Skip to content
      </a>
      <header className="app-header">
        <Link className="brand" to="/overview" aria-label="Prysm home">
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
        </Link>
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
      {control.settings && !setup ? (
        <nav className="main-nav" aria-label="Main navigation">
          <NavLink to="/overview">Overview</NavLink>
          <NavLink to="/preview">Preview</NavLink>
          <NavLink to="/settings">
            Settings{' '}
            {dirty ? (
              <span className="draft-dot" aria-label="unsaved changes" />
            ) : null}
          </NavLink>
        </nav>
      ) : null}
      <main id="content" tabIndex={-1}>
        {control.settings ? (
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
        ) : null}
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
                    <Link to="/settings">Settings</Link>.
                  </p>
                </div>
              </>
            ) : null}
            <div hidden={!setup && page !== 'settings'}>
              <Settings
                settings={control.settings}
                data={control.data}
                disabled={
                  !online ||
                  control.busy ||
                  control.data?.calibration_active === true
                }
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
export function App() {
  return (
    <Routes>
      <Route path="/calibrate/:pairing" element={<CalibrationPage />} />
      <Route path="/overview" element={<Dashboard page="overview" />} />
      <Route path="/preview" element={<Dashboard page="preview" />} />
      <Route path="/settings" element={<Dashboard page="settings" />} />
      <Route path="*" element={<Navigate to="/overview" replace />} />
    </Routes>
  );
}
// Preserve QR links from TVs running the previous app version.
if (window.location.hash.startsWith('#calibrate=')) {
  window.history.replaceState(
    null,
    '',
    `#/calibrate/${encodeURIComponent(window.location.hash.slice(11))}`,
  );
}
createRoot(document.getElementById('root')!).render(
  <HashRouter>
    <App />
  </HashRouter>,
);
