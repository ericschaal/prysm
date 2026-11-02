import { useEffect, useState } from 'react';
import { ApiError, type Config, type StateResponse } from './api';

type Processing = Config['processing'];
const numeric = [
  { key: 'brightness_percent', label: 'Brightness', min: 0, max: 100, step: 1, unit: '%', help: 'Scale the final light colors. 0% turns screen colors black.' },
  { key: 'shadow_lift_percent', label: 'Shadow lift', min: 0, max: 100, step: 1, unit: '%', help: 'Bring out dark-scene colors while keeping black off.' },
  { key: 'smoothing_seconds', label: 'Smoothing duration', min: 0, max: 3.4028234663852886e38, step: 'any', unit: 'seconds', help: 'Ease changes over time. Set to 0 to disable smoothing.' },
  { key: 'sample_density', label: 'Sample density', min: 1, max: 4294967295, step: 1, unit: 'samples / 1000 px', help: 'Higher values sample more positions and cost more processing time. Sampling is capped at one per pixel.' },
  { key: 'spatial_reach_percent', label: 'Spatial reach', min: 1, max: 100, step: 1, unit: '%', help: 'How far colors inside the picture influence each edge.' },
] as const;
type NumericKey = typeof numeric[number]['key'];
const equal = (a: Processing, b: Processing) => JSON.stringify(a) === JSON.stringify(b);

export function Settings({ config, data, disabled, save, onDirty }: {
  config: Config; data: StateResponse | null; disabled: boolean;
  save: (config: Config) => Promise<Config>; onDirty: (dirty: boolean) => void;
}) {
  const [base, setBase] = useState(config);
  const [draft, setDraft] = useState<Processing>(config.processing);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [conflict, setConflict] = useState(false);
  const [reset, setReset] = useState(0);
  const [invalid, setInvalid] = useState<Partial<Record<NumericKey, string>>>({});
  const dirty = !equal(draft, base.processing) || Object.keys(invalid).length > 0;
  const externalChange = config.revision !== base.revision;
  useEffect(() => { onDirty(dirty); }, [dirty, onDirty]);
  useEffect(() => {
    if (!dirty && !saving && config.revision !== base.revision) { setBase(config); setDraft(config.processing); }
  }, [config, dirty, saving, base.revision]);
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);
  function discard() { setBase(config); setDraft(config.processing); setError(''); setConflict(false); setInvalid({}); setReset(previous => previous + 1); }
  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!dirty || disabled || saving || Object.keys(invalid).length || externalChange || conflict) return;
    setSaving(true); setError('');
    try {
      const saved = await save({ ...base, processing: draft });
      setBase(saved); setDraft(saved.processing); setConflict(false);
    } catch (error) {
      setError(error instanceof Error ? error.message : 'Could not save settings.');
      if (error instanceof ApiError && error.status === 409) setConflict(true);
    } finally { setSaving(false); }
  }
  function numberField(field: typeof numeric[number]) {
    const { key, label, min, max, step, unit, help } = field;
    return <div className="setting" key={key}>
      <label htmlFor={key}>{label} <span className="unit">{unit}</span></label>
      <p id={`${key}-help`}>{help}<span className="range-help">Range: {min}–{new Intl.NumberFormat(undefined, { notation: max > 1e12 ? 'scientific' : 'standard', maximumFractionDigits: 16 }).format(max)} {unit}.</span></p>
      <input id={key} name={key} autoComplete="off" type="number" min={min} max={max} step={step} defaultValue={draft[key]}
        key={`${base.revision}-${reset}-${key}`} disabled={saving} aria-invalid={!!invalid[key]}
        aria-describedby={`${key}-help${invalid[key] ? ` ${key}-error` : ''}`}
        onChange={event => {
          const value = event.target.valueAsNumber;
          const valid = event.target.validity.valid && Number.isFinite(value) && value >= min && value <= max;
          setInvalid(previous => { const next = { ...previous }; if (valid) delete next[key]; else next[key] = `Enter ${step === 1 ? 'a whole number' : 'a number'} from ${min} to ${max}.`; return next; });
          if (valid) setDraft(previous => ({ ...previous, [key]: value }));
          setError('');
        }} />
      {key === 'smoothing_seconds' ? <button type="button" className="text-button" disabled={saving} onClick={() => {
        setDraft(previous => ({ ...previous, smoothing_seconds: 0 }));
        const input = document.getElementById(key) as HTMLInputElement; input.value = '0';
        setInvalid(previous => { const next = { ...previous }; delete next.smoothing_seconds; return next; });
      }}>Disable smoothing</button> : null}
      {invalid[key] ? <p id={`${key}-error`} className="error" role="alert">{invalid[key]}</p> : null}
    </div>;
  }
  const applied = data?.applied_revision === config.revision;
  const active = data?.pipeline.phase === 'running' || data?.pipeline.phase === 'starting';
  let status = dirty ? 'Unsaved changes' : saving ? 'Saving…' : data?.restart_required ? 'Saved · application restart required' : data?.operation.kind === 'failed' ? 'Saved · application failed' : applied ? `Applied · revision ${config.revision}` : active ? `Saved · applying revision ${config.revision}` : `Saved · revision ${config.revision} applies when screen sync starts`;
  if (saving) status = 'Saving…';
  return <form className="settings card" onSubmit={submit} aria-labelledby="settings-heading" aria-busy={saving} noValidate>
    <div className="section-heading"><div><p className="eyebrow">LIVE PROCESSING</p><h2 id="settings-heading">Tune the picture</h2></div><span className={`badge ${dirty ? 'amber' : ''}`}>{dirty ? 'Draft' : 'Saved'}</span></div>
    <p className="intro-copy">Save & apply updates these settings between frames, without restarting capture or lighting output. Temporal history is preserved where the changed setting allows it.</p>
    <div className="picture-settings">{numeric.slice(0, 3).map(numberField)}
      <div className="setting checkbox-setting"><label htmlFor="remove_black_bars"><input id="remove_black_bars" name="remove_black_bars" autoComplete="off" type="checkbox" checked={draft.remove_black_bars} disabled={saving} aria-describedby="bars-help" onChange={event => setDraft(previous => ({ ...previous, remove_black_bars: event.target.checked }))} /> Remove black bars</label><p id="bars-help">Detect the active picture before sampling colors.</p></div>
    </div>
    <section className="advanced" aria-labelledby="advanced-heading"><h3 id="advanced-heading">Quality & performance</h3><p>Advanced controls. Higher density adds work; reach changes the sampled area.</p><div className="advanced-fields">{numeric.slice(3).map(numberField)}</div></section>
    {conflict || (externalChange && dirty) ? <div className="notice warning" role="alert"><strong>Saved settings changed elsewhere.</strong><p>Your draft is preserved. Review the latest saved values below, then reload to discard this draft before editing again.</p><dl className="conflict-values">{numeric.map(field => <div key={field.key}><dt>{field.label}</dt><dd>Saved: {config.processing[field.key]} · draft: {draft[field.key]}</dd></div>)}<div><dt>Remove black bars</dt><dd>Saved: {config.processing.remove_black_bars ? 'On' : 'Off'} · draft: {draft.remove_black_bars ? 'On' : 'Off'}</dd></div></dl><button type="button" onClick={discard} disabled={saving}>Reload saved settings & discard draft</button></div> : null}
    {error ? <p className="error" role="alert">{error} Your draft is preserved.</p> : null}
    <div className="save-bar"><p id="settings-status" role="status">{status}</p><div className="button-row"><button type="button" onClick={discard} disabled={!dirty || saving}>Discard</button><button className="primary" type="submit" disabled={!dirty || disabled || saving || !!Object.keys(invalid).length || externalChange || conflict}>{saving ? 'Saving…' : 'Save & apply'}</button></div></div>
    <p className="revision-note">Saved revision {config.revision} · processor revision {data?.applied_revision ?? 'not yet applied'}. A save confirms persistence; operation status confirms application.</p>
  </form>;
}
