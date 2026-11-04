import { useCallback, useState } from 'react';
import Calibration from './components/Calibration';
import Header from './components/Header';
import CaptureOverview from './components/CaptureOverview';
import Settings from './components/Settings';
import CaptureDialog from './components/CaptureDialog';
import { capturePresentation } from './capturePresentation';
import { useCaptureService } from './hooks/useCaptureService';
import { useRemoteNavigation } from './hooks/useRemoteNavigation';

export default function App() {
  const capture = useCaptureService();
  // Mode and capability changes invalidate any open mode-specific dialog or calibration session.
  const screenKey = [
    capture.embeddedAvailable,
    capture.saved?.location ?? 'remote',
  ].join('-');
  return <CaptureScreen key={screenKey} capture={capture} />;
}

function CaptureScreen({
  capture,
}: {
  capture: ReturnType<typeof useCaptureService>;
}) {
  const [calibrating, setCalibrating] = useState(false);
  const closeCalibration = useCallback(() => setCalibrating(false), []);
  const calibrationOrigin =
    calibrating &&
    capture.saved?.location !== 'embedded' &&
    capture.saved?.instanceUrl;
  const navigation = useRemoteNavigation(!!calibrationOrigin);
  const presentation = capturePresentation(
    capture.online,
    capture.status,
    capture.draft,
    capture.dirty,
  );

  if (calibrationOrigin)
    return <Calibration origin={calibrationOrigin} close={closeCalibration} />;

  return (
    <div
      className={presentation.ready ? 'ready' : undefined}
      onMouseOver={navigation.onMouseOver}
      onFocus={navigation.onFocus}
    >
      <main id="app" aria-hidden={navigation.dialog ? true : undefined}>
        <Header />
        <div className="home-content">
          <div className="layout">
            <CaptureOverview
              {...presentation}
              online={capture.online}
              changing={capture.changing}
              primary={() => capture.primary(navigation.openDialog)}
              openService={() => navigation.openDialog('service')}
            />
            <Settings
              online={capture.online}
              saved={capture.saved}
              embeddedAvailable={capture.embeddedAvailable}
              draft={capture.draft}
              dirty={capture.dirty}
              changing={capture.changing}
              edit={capture.edit}
              calibrate={() =>
                capture.saved?.instanceUrl
                  ? setCalibrating(true)
                  : navigation.openDialog('receiver')
              }
              saveSettings={() => capture.saveSettings(navigation.openDialog)}
              openDialog={navigation.openDialog}
            />
          </div>
          <p
            id="notice"
            className={'notice' + (capture.error ? ' error' : '')}
            role={capture.error ? 'alert' : 'status'}
            aria-live="polite"
          >
            {capture.notice}
          </p>
        </div>
        <footer className="key-help">
          <span>
            <b>↑ ↓ ← →</b> Move
          </span>
          <span>
            <b>OK</b> Choose
          </span>
          <span>
            <b>Back</b> Return
          </span>
        </footer>
      </main>
      {navigation.dialog ? (
        <CaptureDialog
          key={navigation.dialog}
          dialog={navigation.dialog}
          modal={navigation.modal}
          draft={capture.draft}
          status={capture.status}
          edit={capture.edit}
          openDialog={navigation.openDialog}
          closeDialog={navigation.closeDialog}
          dismiss={navigation.dismiss}
          setup={capture.setup}
          cleanup={capture.cleanup}
        />
      ) : null}
    </div>
  );
}
