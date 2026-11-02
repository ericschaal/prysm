import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { useControl } from './useControl';
import { Overview } from './Overview';
import { Settings } from './Settings';
import { Preview } from './Preview';
import './style.css';

function App() {
  const control = useControl();
  const [page, setPage] = useState<'overview' | 'tuning'>(window.location.hash === '#tuning' ? 'tuning' : 'overview');
  useEffect(() => {
    const navigate = () => setPage(window.location.hash === '#tuning' ? 'tuning' : 'overview');
    window.addEventListener('hashchange', navigate);
    return () => window.removeEventListener('hashchange', navigate);
  }, []);
  const [dirty, setDirty] = useState(false);
  const online = !!control.data && !control.connectionError;
  return <div className="app-shell"><a className="skip-link" href="#content">Skip to content</a><header className="app-header"><a className="brand" href="#overview" aria-label="Prysm home"><svg viewBox="0 0 32 32" fill="none" aria-hidden="true"><path d="m16 3 13 24H3L16 3Z" stroke="currentColor" strokeWidth="2" /><path d="m16 3 4 24M3 27l15-12 11 12" stroke="currentColor" strokeWidth="1.5" /></svg><span>prysm<span className="brand-dot">.</span></span></a><span className={`connection ${online ? 'ready' : 'attention'}`} role="status"><i />{control.connectionError ? 'Offline' : online ? 'Connected' : 'Connecting'}</span></header>
    <nav className="main-nav" aria-label="Main navigation"><a href="#overview" aria-current={page === 'overview' ? 'page' : undefined}>Overview</a><a href="#tuning" aria-current={page === 'tuning' ? 'page' : undefined}>Preview & tuning {dirty ? <span className="draft-dot" aria-label="unsaved changes" /> : null}</a></nav>
    <main id="content"><div className="page-intro"><p className="eyebrow">{page === 'overview' ? 'ROOM CONTROL' : 'PICTURE & COLOR'}</p><h1>{page === 'overview' ? 'Light, in sync.' : 'Make the picture feel right.'}</h1><p>{page === 'overview' ? 'Control your lights and see what Prysm is doing.' : 'Inspect the picture and apply live processing changes together.'}</p></div>
      <div hidden={page !== 'overview'}><Overview {...control} /></div>
      <div hidden={page !== 'tuning'} className="tuning-grid"><div className="preview-column"><Preview active={page === 'tuning'} data={control.data} />{control.connectionError ? <p className="notice error" role="alert">Controller connection lost. Your draft is preserved. {control.connectionError}</p> : null}{control.data?.restart_required ? <p className="notice warning">An external configuration change requires an application restart. Live settings can still be saved; power/source changes wait for restart.</p> : null}{control.data?.operation.kind === 'failed' ? <p className="notice error" role="alert">Application failed: {control.data.operation.error.message}. Saved values may differ from the processor; check the applied revision.</p> : null}</div>{control.config ? <Settings config={control.config} data={control.data} disabled={!online || control.busy} save={control.saveConfig} onDirty={setDirty} /> : <section className="card"><h2>Loading settings</h2><p>{control.connectionError || 'Waiting for configuration from Prysm…'}</p><button type="button" onClick={() => void control.poll()}>Check connection</button></section>}</div>
    </main><footer>One picture. One lighting output.<span>Preview observes capture; opening it never turns the lights on.</span></footer></div>;
}
createRoot(document.getElementById('root')!).render(<App />);
