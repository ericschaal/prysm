import type { Config } from '../service';
import type { Dialog } from '../hooks/useRemoteNavigation';
import { embeddedBuild } from '../processingLocation';

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
  embeddedAvailable: boolean;
  calibrationAvailable: boolean;
  draft: Config;
  dirty: boolean;
  changing: boolean;
  openDialog: (dialog: Dialog) => void;
  edit: <K extends keyof Config>(key: K, value: Config[K]) => void;
  saveSettings: () => void;
  calibrate: () => void;
}

export default function Settings({
  online,
  saved,
  embeddedAvailable,
  calibrationAvailable,
  draft,
  dirty,
  changing,
  openDialog,
  edit,
  saveSettings,
  calibrate,
}: Props) {
  let savedLabel = 'On this TV';
  if (saved) savedLabel = dirty ? 'Unsaved changes' : 'Saved';
  const embedded =
    embeddedBuild && embeddedAvailable && draft.location === 'embedded';
  const showCalibration =
    (saved?.location === 'embedded') === embedded && calibrationAvailable;
  const rows: [Dialog, string, string][] = [];
  if (embeddedBuild && embeddedAvailable) {
    rows.push([
      'location',
      'Processing location',
      embedded ? 'On this TV (experimental)' : 'Remote instance',
    ]);
  }
  if (!embedded) {
    rows.push([
      'receiver',
      'Prysm instance URL',
      draft.instanceUrl || 'Add address',
    ]);
  }
  rows.push(['source', 'Capture', names.source[draft.source]]);
  if (!embedded)
    rows.push([
      'compression',
      'Compression',
      names.compression[draft.compression],
    ]);
  rows.push(['autostart', 'Enable with TV', draft.autostart ? 'On' : 'Off']);

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
        <button
          id="save"
          className="secondary"
          hidden={!dirty}
          disabled={!online || !dirty || changing}
          data-down={rows[0]?.[0]}
          data-left="primary"
          onClick={saveSettings}
        >
          Save changes
        </button>
      </div>
      <div className="setting-list">
        {rows.map(([key, title, value], index) => (
          <button
            key={key}
            id={key}
            className="setting"
            aria-haspopup="dialog"
            data-up={rows[index - 1]?.[0] ?? 'save'}
            data-down={
              rows[index + 1]?.[0] ??
              (showCalibration ? 'calibrate' : 'metrics')
            }
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
        {showCalibration && (
          <button
            id="calibrate"
            className="setting"
            data-up="autostart"
            data-down="metrics"
            data-left={
              online === null || changing ? 'service-options' : 'primary'
            }
            onClick={calibrate}
          >
            <span className="setting-label">Calibrate Colors</span>
            <span className="chevron" aria-hidden="true">
              ›
            </span>
          </button>
        )}
        <button
          id="metrics"
          className="setting"
          role="checkbox"
          aria-checked={draft.metrics}
          aria-describedby="metrics-description"
          data-up={showCalibration ? 'calibrate' : 'autostart'}
          data-down="save"
          data-left="primary"
          disabled={!online || changing}
          onClick={() => edit('metrics', !draft.metrics)}
        >
          <span>
            <span className="setting-label">Prometheus metrics</span>
            <span id="metrics-description" className="setting-detail">
              Port 9101
            </span>
          </span>
          <span className="tickbox" aria-hidden="true" />
        </button>
      </div>
    </section>
  );
}
