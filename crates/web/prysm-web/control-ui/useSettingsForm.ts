import { useEffect, useState } from 'react';
import { ApiError, type SettingsValues, type SettingsView } from './api';
import {
  draftFrom,
  equal,
  settingsFrom,
  validate,
  type SettingsDraft,
} from './settings-draft';

export function useSettingsForm(
  settings: SettingsView,
  save: (revision: string, changes: SettingsValues) => Promise<SettingsView>,
  onDirty: (dirty: boolean) => void,
) {
  const [base, setBase] = useState(settings);
  const [draft, setDraft] = useState(() => draftFrom(settings.settings));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [conflict, setConflict] = useState(false);
  const dirty = !equal(draft, draftFrom(base.settings));
  const externalChange = settings.revision !== base.revision;
  const errors = validate(draft);
  useEffect(() => {
    onDirty(dirty);
  }, [dirty, onDirty]);
  if (!dirty && !saving && externalChange) {
    setBase(settings);
    setDraft(draftFrom(settings.settings));
  }
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);

  function change<K extends keyof SettingsDraft>(
    group: K,
    value: SettingsDraft[K],
  ) {
    setDraft((previous) => ({ ...previous, [group]: value }));
    setError('');
  }
  function discard() {
    setBase(settings);
    setDraft(draftFrom(settings.settings));
    setError('');
    setConflict(false);
  }
  async function submit(): Promise<SettingsView | null> {
    if (saving || externalChange || conflict) return null;
    if (Object.keys(errors).length) {
      setError('Correct the highlighted fields before saving.');
      return null;
    }
    const changes = settingsFrom(draft);
    setSaving(true);
    setError('');
    try {
      const saved = await save(base.revision, changes);
      setBase(saved);
      setDraft(draftFrom(saved.settings));
      setConflict(false);
      return saved;
    } catch (failure) {
      setError(
        failure instanceof Error ? failure.message : 'Could not save settings.',
      );
      if (failure instanceof ApiError && failure.status === 409)
        setConflict(true);
      return null;
    } finally {
      setSaving(false);
    }
  }
  return {
    base,
    draft,
    errors,
    dirty,
    saving,
    error,
    conflict,
    externalChange,
    change,
    discard,
    submit,
  };
}
