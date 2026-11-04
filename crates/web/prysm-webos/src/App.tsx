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
  const [calibrating, setCalibrating] = useState(false);
  const closeCalibration = useCallback(() => setCalibrating(false), []);
  const navigation = useRemoteNavigation(calibrating);
  const capture = useCaptureService(() => navigation.openDialog('receiver'));
  const presentation = capturePresentation(
    capture.online,
    capture.status,
    capture.draft,
    capture.dirty,
  );

  if (calibrating)
    return (
      <Calibration
        receiver={capture.saved?.receiver ?? ''}
        close={closeCalibration}
      />
    );

  return (
    <div
      className={presentation.ready ? 'ready' : undefined}
      onMouseOver={navigation.onMouseOver}
      onFocus={navigation.onFocus}
    >
      <main id="app" aria-hidden={navigation.dialog ? true : undefined}>
        <Header />
        <div className="layout">
          <CaptureOverview
            {...presentation}
            online={capture.online}
            changing={capture.changing}
            primary={capture.primary}
            calibrate={() => setCalibrating(true)}
            openService={() => navigation.openDialog('service')}
          />
          <Settings
            online={capture.online}
            saved={capture.saved}
            draft={capture.draft}
            dirty={capture.dirty}
            changing={capture.changing}
            edit={capture.edit}
            saveSettings={capture.saveSettings}
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
