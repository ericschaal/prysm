import type { Config } from '../service';
import type { Dialog } from '../hooks/useRemoteNavigation';

const names = {
  source: { both: 'Video + TV menus', video: 'Video only' },
  compression: {
    lz4: 'LZ4',
    none: 'None',
    'h264-software': 'H.264 (software)',
    'h264-hardware': 'H.264 (hardware)',
  },
};
interface Props {
  online: boolean | null;
  saved: Config | null;
  draft: Config;
  dirty: boolean;
  changing: boolean;
  openDialog: (dialog: Dialog) => void;
  edit: <K extends keyof Config>(key: K, value: Config[K]) => void;
  saveSettings: () => void;
}

export default function Settings({
  online,
  saved,
  draft,
  dirty,
  changing,
  openDialog,
  edit,
  saveSettings,
}: Props) {
  let savedLabel = 'On this TV';
  if (saved) savedLabel = dirty ? 'Unsaved changes' : 'Saved';
  const rows = [
    ['receiver', 'Prysm receiver', draft.receiver || 'Add address'],
    ['source', 'Capture', names.source[draft.source]],
    ['compression', 'Compression', names.compression[draft.compression]],
    ['autostart', 'Enable with TV', draft.autostart ? 'On' : 'Off'],
  ] as const;

  return (
    <section className="settings" aria-labelledby="settings-title">
      <div className="section-heading">
        <h2 id="settings-title">Settings</h2>
        <span
          id="saved-state"
          className={saved && dirty ? 'unsaved' : undefined}
        >
          {savedLabel}
        </span>
      </div>
      <div className="setting-list">
        {rows.map(([key, title, value], index) => (
          <button
            key={key}
            id={key}
            className="setting"
            aria-haspopup="dialog"
            data-up={rows[index - 1]?.[0] ?? 'receiver'}
            data-down={rows[index + 1]?.[0] ?? 'metrics'}
            data-left="primary"
            disabled={!online || changing}
            onClick={() => openDialog(key)}
          >
            <span className="setting-label">{title}</span>
            <span className="setting-selection">
              <span
                id={key + '-value'}
                className={
                  'setting-value' + (key === 'receiver' ? ' address' : '')
                }
              >
                {value}
              </span>
              <span className="chevron" aria-hidden="true">
                ›
              </span>
            </span>
          </button>
        ))}
        <button
          id="metrics"
          className="setting"
          role="checkbox"
          aria-checked={draft.metrics}
          aria-describedby="metrics-description"
          data-up="autostart"
          data-down="save"
          data-left="primary"
          disabled={!online || changing}
          onClick={() => edit('metrics', !draft.metrics)}
        >
          <span>
            <span className="setting-label">Prometheus metrics</span>
            <span id="metrics-description" className="setting-detail">
              Port 9101 · while the sender is enabled.
            </span>
          </span>
          <span className="tickbox" aria-hidden="true" />
        </button>
      </div>
      <div className="settings-footer">
        <button
          id="save"
          className="secondary"
          data-up="metrics"
          data-left="service-options"
          disabled={!online || !dirty || changing}
          onClick={saveSettings}
        >
          Save changes
        </button>
      </div>
    </section>
  );
}
