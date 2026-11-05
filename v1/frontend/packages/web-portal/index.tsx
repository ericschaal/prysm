import type { ApplicationRuntime } from '@prysm/application-state';
import { useState, type ComponentType } from 'react';
import type { Accepted } from '@prysm/application-client';
import {
  useApplication,
  useApplicationDispatch,
} from '@prysm/application-react';

export interface SettingsPanelProps<S> {
  settings: S | null;
  disabled: boolean;
  /** The portal displays acceptance/errors; acceptance does not mean applied. */
  onSubmit: (settings: S) => Promise<void>;
}

export interface WebPortalProps<C, O> {
  application: ApplicationRuntime<C, O>;
  captureSettings: ComponentType<SettingsPanelProps<NoInfer<C>>>;
  outputSettings: ComponentType<SettingsPanelProps<NoInfer<O>>>;
}

export function WebPortal<C, O>({
  application,
  captureSettings: CaptureSettings,
  outputSettings: OutputSettings,
}: WebPortalProps<C, O>) {
  const connection = useApplication(application);
  const dispatch = useApplicationDispatch<C, O>();
  const snapshot = connection.snapshot;
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  async function submit(command: () => Promise<Accepted>) {
    setPending(true);
    setError('');
    setMessage('');
    try {
      const accepted = await command();
      setMessage(
        `Accepted operation ${accepted.operation.request}; application completion is separate.`,
      );
    } catch (failure) {
      setError(
        failure instanceof Error
          ? failure.message
          : 'Settings submission failed.',
      );
    } finally {
      setPending(false);
    }
  }

  if (!snapshot)
    return (
      <main>
        <h1>Prysm</h1>
        <p role="status">{connection.error ?? connection.status}</p>
      </main>
    );

  return (
    <main>
      <h1>Prysm</h1>
      <p>Settings revision: {snapshot.revision}</p>
      <section aria-label="Capture settings">
        <CaptureSettings
          settings={snapshot.capture}
          disabled={pending}
          onSubmit={(settings) =>
            submit(() =>
              dispatch(
                application.commands.configureCapture(
                  snapshot.revision,
                  settings,
                ),
              ),
            )
          }
        />
      </section>
      <section aria-label="Output settings">
        <OutputSettings
          settings={snapshot.output}
          disabled={pending}
          onSubmit={(settings) =>
            submit(() =>
              dispatch(
                application.commands.configureOutput(
                  snapshot.revision,
                  settings,
                ),
              ),
            )
          }
        />
      </section>
      <p role="status">{pending ? 'Submitting settings…' : message}</p>
      {error ? <p role="alert">{error}</p> : null}
    </main>
  );
}
