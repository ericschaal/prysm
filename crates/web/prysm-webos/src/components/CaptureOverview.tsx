import { embeddedBuild } from '../processingLocation';

interface Props {
  online: boolean | null;
  title: string;
  description: string;
  label: string;
  attention: boolean;
  changing: boolean;
  portalUrl?: string;
  primary: () => void;
  openService: () => void;
}

export default function CaptureOverview({
  online,
  title,
  description,
  label,
  attention,
  changing,
  portalUrl,
  primary,
  openService,
}: Props) {
  return (
    <section
      className={'overview' + (attention ? ' attention' : '')}
      aria-labelledby="status-title"
    >
      <p className="section-label">Capture status</p>
      <div id="connection" role="status" aria-live="polite">
        <h2 id="status-title">{title}</h2>
      </div>
      <p id="status-description" className="description">
        {description}
      </p>
      {embeddedBuild && portalUrl && (
        <div className="portal-address" id="portal-address">
          <strong>Prysm web portal</strong>
          <code>{portalUrl}</code>
          Open this address on your phone or computer to configure your lights.
        </div>
      )}
      <div className="main-actions">
        <button
          id="primary"
          className="primary"
          data-down="service-options"
          data-right="receiver"
          disabled={online === null || changing}
          onClick={primary}
        >
          {changing ? 'Please wait…' : label}
        </button>
        <button
          id="service-options"
          className="quiet"
          data-up="primary"
          data-right="receiver"
          onClick={openService}
        >
          Service options
        </button>
      </div>
      <p className="capture-hint">
        Capture stays enabled when you close the app.
      </p>
    </section>
  );
}
