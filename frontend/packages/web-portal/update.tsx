import { useRouteError } from 'react-router';

const reload = () => window.location.reload();

/** Shown on every page once Prysm was updated: this page is the old one. */
export function UpdatedNotice() {
  return (
    <div className="notice updated" role="status">
      <p>Prysm was updated.</p>
      <button type="button" onClick={reload}>
        Reload
      </button>
    </div>
  );
}

/** In place of a page that failed to draw: what happened, and the way on. */
export function PortalError() {
  const error = useRouteError();
  return (
    <div className="portal">
      <title>Prysm</title>
      <main id="content" className="portal-flow">
        <section className="connecting problem" role="alert">
          <h1 className="visually-hidden">Prysm</h1>
          <h2>Something went wrong</h2>
          <p>
            This page hit a problem. Your lights and saved settings are not
            affected. Reload to continue.
          </p>
          <button type="button" className="primary" onClick={reload}>
            Reload
          </button>
          <p className="detail">
            {error instanceof Error ? error.message : String(error)}
          </p>
        </section>
      </main>
    </div>
  );
}
