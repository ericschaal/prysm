import { useNavigate } from 'react-router';
import { useEffect, useRef, useState } from 'react';
import type { SettingsValues, SettingsView, StateResponse } from './api';
import {
  InputFields,
  OutputFields,
  ProcessingFields,
  SourceField,
} from './SettingsFields';
import { ProfileSettings } from './ProfileSettings';
import { LedLayout } from './LedLayout';
import { inputLabels } from './settings-draft';
import { useSettingsForm } from './useSettingsForm';

const steps = ['Video source', 'Your lights', 'Review & finish'];
const stepDescriptions = [
  'Choose where your picture comes from. You can fine-tune capture later.',
  'Connect your WLED controller and map the LEDs around your screen.',
  'Check your setup. Saving keeps your lights as they are.',
];

export function Settings({
  settings,
  data,
  disabled,
  save,
  onDirty,
}: {
  settings: SettingsView;
  data: StateResponse | null;
  disabled: boolean;
  save: (revision: string, changes: SettingsValues) => Promise<SettingsView>;
  onDirty: (dirty: boolean) => void;
}) {
  const navigate = useNavigate();
  const form = useSettingsForm(settings, save, onDirty);
  const [discardCount, setDiscardCount] = useState(0);
  const [attempted, setAttempted] = useState(false);
  const [step, setStep] = useState(0);
  const formElementRef = useRef<HTMLFormElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const errorSummaryRef = useRef<HTMLDivElement>(null);
  const setup = settings.missing_required.length > 0;
  const { draft } = form;
  useEffect(() => {
    if (setup) headingRef.current?.focus();
  }, [step, setup]);
  useEffect(() => {
    if (form.error) errorSummaryRef.current?.focus();
  }, [form.error]);
  // A later reset of required settings starts a fresh guided setup.
  const [previousSetup, setPreviousSetup] = useState(setup);
  if (previousSetup !== setup) {
    setPreviousSetup(setup);
    if (setup) setStep(0);
  }
  function discard() {
    form.discard();
    setDiscardCount((count) => count + 1);
    setAttempted(false);
  }
  function focusError(prefix = '') {
    const field = Object.keys(form.errors).find((path) =>
      path.startsWith(prefix),
    );
    if (!field) return false;
    const input = formElementRef.current?.querySelector<HTMLElement>(
      `[name="${field}"]`,
    );
    const details = input?.closest('details');
    if (details) details.open = true;
    input?.focus();
    return true;
  }
  function goToStep(next: number) {
    setAttempted(false);
    setStep(next);
  }
  function next() {
    setAttempted(true);
    const prefix = step === 0 ? 'input.' : 'output.';
    if (focusError(prefix)) return;
    goToStep(step + 1);
  }
  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (setup && step < 2) {
      next();
      return;
    }
    setAttempted(true);
    if (disabled || data?.calibration_active) return;
    if (focusError()) return;
    const saved = await form.submit();
    if (saved) {
      setAttempted(false);
      if (setup && !saved.missing_required.length) void navigate('/overview');
    }
  }
  const fields = {
    settings,
    errors: form.errors,
    attempted,
    disabled: form.saving || disabled || data?.calibration_active === true,
  };
  function settingsStatus() {
    if (setup) return 'Nothing is saved until you finish.';
    if (form.saving) return 'Saving…';
    if (form.dirty) return 'Unsaved changes';
    if (data?.operation.kind === 'failed') return 'Saved · application failed';
    if (data?.applied_revision === settings.revision)
      return 'Applied to your lights';
    return 'Saved · ready for the next screen sync';
  }
  const setupAction = step < steps.length - 1 ? 'Next' : 'Finish setup';
  const unavailable =
    disabled || form.saving || form.externalChange || form.conflict;
  return (
    <form
      ref={formElementRef}
      className={`settings card${setup ? ' setup-wizard' : ''}`}
      onSubmit={(event) => void submit(event)}
      aria-labelledby="settings-heading"
      aria-busy={form.saving}
      noValidate
    >
      {data?.calibration_active ? (
        <p role="status">
          Calibration in progress—finish or exit on the TV to change settings.
        </p>
      ) : null}
      {setup ? (
        <ol className="setup-steps" aria-label="Setup progress">
          {steps.map((label, index) => (
            <li
              key={label}
              className={index < step ? 'complete' : undefined}
              aria-current={step === index ? 'step' : undefined}
            >
              <span className="step-number" aria-hidden="true">
                {index < step ? '✓' : index + 1}
              </span>
              <span>{label}</span>
            </li>
          ))}
        </ol>
      ) : null}
      <div className="section-heading">
        <div>
          <p className="eyebrow">
            {setup
              ? `STEP ${step + 1} OF ${steps.length}`
              : 'YOUR CONFIGURATION'}
          </p>
          <h2 id="settings-heading" ref={headingRef} tabIndex={-1}>
            {setup ? steps[step] : 'Settings'}
          </h2>
        </div>
        {!setup ? (
          <span className={`badge ${form.dirty ? 'amber' : ''}`}>
            {form.dirty ? 'Draft' : 'Saved'}
          </span>
        ) : null}
      </div>
      <p className="intro-copy">
        {setup
          ? stepDescriptions[step]
          : 'Configure capture, lights, processing, and color source. Changes are saved together. Saving does not turn lights on.'}
      </p>
      {!setup || step === 0 ? (
        <InputFields
          {...fields}
          value={draft.input}
          onChange={(value) => form.change('input', value)}
        />
      ) : null}
      {!setup || step === 1 ? (
        <OutputFields
          {...fields}
          value={draft.output}
          onChange={(value) => form.change('output', value)}
        />
      ) : null}
      {setup && step === 2 ? (
        <>
          <section className="setup-review" aria-label="Setup review">
            <div className="review-details">
              <div className="review-item">
                <div className="review-heading">
                  <h3>Video source</h3>
                  <button
                    type="button"
                    aria-label="Edit video source"
                    onClick={() => goToStep(0)}
                    disabled={form.saving}
                  >
                    Edit
                  </button>
                </div>
                <p className="review-value">{inputLabels[draft.input.type]}</p>
                <p>
                  {draft.input.width} × {draft.input.height} ·{' '}
                  {draft.input.refresh_rate_hz} fps
                </p>
                {draft.input.type !== 'webos' ? (
                  <p className="review-address">
                    {draft.input.type === 'device'
                      ? draft.input.device
                      : draft.input.path}
                  </p>
                ) : null}
              </div>
              <div className="review-item">
                <div className="review-heading">
                  <h3>Your lights</h3>
                  <button
                    type="button"
                    aria-label="Edit lights"
                    onClick={() => goToStep(1)}
                    disabled={form.saving}
                  >
                    Edit
                  </button>
                </div>
                <p className="review-value">WLED controller</p>
                <p className="review-address">{draft.output.url}</p>
              </div>
            </div>
            <LedLayout leds={draft.output.leds} />
          </section>
          <p className="setup-note">
            You can finish setup while your devices are offline.
          </p>
        </>
      ) : null}
      {!setup || step === 2 ? (
        <div className={setup ? 'setup-preferences' : undefined}>
          <SourceField
            {...fields}
            value={draft.source}
            onChange={(value) => form.change('source', value)}
          />
          <ProfileSettings
            key={form.base.revision + '/' + discardCount}
            calibration={draft.calibration}
            deleted={draft.deletedProfiles}
            errors={form.errors}
            disabled={fields.disabled}
            rename={(id, name) =>
              form.change('calibration', {
                ...draft.calibration,
                profiles: draft.calibration.profiles.map((p) =>
                  p.id === id ? { ...p, name } : p,
                ),
              })
            }
            markDeleted={(ids) => form.change('deletedProfiles', ids)}
          />
          {setup ? (
            <details className="setup-advanced">
              <summary>
                Picture settings <span>Use defaults or fine-tune</span>
              </summary>
              <ProcessingFields
                {...fields}
                value={draft.processing}
                frameRate={draft.input.refresh_rate_hz}
                onChange={(value) => form.change('processing', value)}
              />
            </details>
          ) : (
            <>
              <ProcessingFields
                {...fields}
                value={draft.processing}
                frameRate={draft.input.refresh_rate_hz}
                onChange={(value) => form.change('processing', value)}
              />
            </>
          )}
        </div>
      ) : null}
      {form.conflict || (form.externalChange && form.dirty) ? (
        <div className="notice warning" role="alert">
          <strong>Saved settings changed elsewhere.</strong>
          <p>
            Your draft is preserved. Reload the current settings before editing
            again.
          </p>
          <button type="button" onClick={discard} disabled={form.saving}>
            Reload saved settings & discard draft
          </button>
        </div>
      ) : null}
      {form.error ? (
        <div
          className="notice error"
          role="alert"
          tabIndex={-1}
          ref={errorSummaryRef}
        >
          {form.error} Your draft is preserved.
        </div>
      ) : null}
      {data?.operation.kind === 'failed' ? (
        <p className="notice error" role="alert">
          Application failed: {data.operation.error.message}. Saved values may
          differ from the processor; check the applied revision.
        </p>
      ) : null}
      <div className="save-bar">
        <p id="settings-status" role="status">
          {settingsStatus()}
        </p>
        <div className="button-row">
          {setup ? (
            <>
              {step > 0 ? (
                <button
                  type="button"
                  disabled={form.saving}
                  onClick={() => goToStep(step - 1)}
                >
                  Back
                </button>
              ) : null}
              <button className="primary" type="submit" disabled={unavailable}>
                {form.saving ? 'Saving…' : setupAction}
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                onClick={discard}
                disabled={!form.dirty || form.saving}
              >
                Discard
              </button>
              <button
                className="primary"
                type="submit"
                disabled={!form.dirty || unavailable}
              >
                {form.saving ? 'Saving…' : 'Save & apply'}
              </button>
            </>
          )}
        </div>
      </div>
    </form>
  );
}
