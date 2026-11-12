import type { ApplicationRuntime } from '@prysm/application-state';
import { useState, type ComponentType } from 'react';
import type { Session } from '@prysm/application-client';
import {
  useApplication,
  useLastCommand,
  useApplicationDispatch,
} from '@prysm/application-react';

export interface SettingsPanelProps<S> {
  /** `null` until this section is configured. */
  settings: S | null;
  disabled: boolean;
  /** Resolves once the settings are saved; rejects with the server's reason. */
  onSubmit: (settings: S) => Promise<void>;
}

export interface WebPortalProps<C, O> {
  application: ApplicationRuntime<C, O>;
  captureSettings: ComponentType<SettingsPanelProps<NoInfer<C>>>;
  outputSettings: ComponentType<SettingsPanelProps<NoInfer<O>>>;
}

function describe(session: Session) {
  return session.state === 'faulted'
    ? `faulted while ${session.phase.replace('_', ' ')}; restart Prysm`
    : session.state;
}

export function WebPortal<C, O>({
  application,
  captureSettings: CaptureSettings,
  outputSettings: OutputSettings,
}: WebPortalProps<C, O>) {
  const connection = useApplication(application);
  const dispatch = useApplicationDispatch<C, O>();
  const snapshot = connection.snapshot;
  const lastCommand = useLastCommand(application);
  const [pending, setPending] = useState(0);

  async function submit(command: () => Promise<unknown>) {
    setPending((count) => count + 1);
    try {
      await command();
    } catch {
      // The failure is shown from the command state below.
    } finally {
      setPending((count) => count - 1);
    }
  }

  if (!snapshot)
    return (
      <main>
        <h1>Prysm</h1>
        <p role="status">{connection.error ?? connection.status}</p>
      </main>
    );

  const revision = snapshot.revision;
  return (
    <main>
      <h1>Prysm</h1>
      <section aria-label="Lighting">
        <button
          disabled={pending > 0}
          onClick={() =>
            void submit(() =>
              dispatch(application.commands.setTarget('processed')),
            )
          }
        >
          Turn lighting on
        </button>
        <button
          disabled={pending > 0}
          onClick={() =>
            void submit(() => dispatch(application.commands.setTarget('off')))
          }
        >
          Turn lighting off
        </button>
        <p>
          Target: {snapshot.target} · Session: {describe(snapshot.session)}
        </p>
        {snapshot.failure ? (
          <p role="alert">Lighting failed: {snapshot.failure.message}</p>
        ) : null}
      </section>
      <p>Settings revision: {revision}</p>
      <section aria-label="Capture settings">
        <CaptureSettings
          settings={snapshot.settings.capture ?? null}
          disabled={pending > 0}
          onSubmit={(settings) =>
            submit(() =>
              dispatch(application.commands.configureCapture(revision, settings)),
            )
          }
        />
      </section>
      <section aria-label="Output settings">
        <OutputSettings
          settings={snapshot.settings.output ?? null}
          disabled={pending > 0}
          onSubmit={(settings) =>
            submit(() =>
              dispatch(application.commands.configureOutput(revision, settings)),
            )
          }
        />
      </section>
      <p role="status">
        {lastCommand?.phase === 'submitting'
          ? 'Working…'
          : lastCommand?.phase === 'succeeded'
            ? 'Done.'
            : ''}
      </p>
      {lastCommand?.failure ? (
        <p role="alert">
          {lastCommand.failure.field
            ? `${lastCommand.failure.field}: ${lastCommand.failure.message}`
            : lastCommand.failure.message}
        </p>
      ) : null}
      {snapshot.surface_failures.map((failure, index) => (
        <p key={index}>Surface stopped: {failure.message}</p>
      ))}
    </main>
  );
}
