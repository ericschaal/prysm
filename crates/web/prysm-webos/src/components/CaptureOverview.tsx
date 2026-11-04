interface Props {
  online: boolean | null;
  title: string;
  description: string;
  label: string;
  attention: boolean;
  changing: boolean;
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
      <p className="capture-hint">Capture stays enabled when you close the app.</p>
    </section>
  );
}
