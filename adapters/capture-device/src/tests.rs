use super::*;
use crate::source::{Capture, CaptureResult, Deadlines, DeviceFrames};
use prysm_capture_device_sys::{CaptureError, CaptureSink};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{AdapterSettings, Error, Registry, Settings, chain};
use std::error::Error as _;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(5);
const SHORT: Duration = Duration::from_millis(50);
const TEST: Deadlines = Deadlines {
    ready: LIMIT,
    release: LIMIT,
};

fn frame(value: u8) -> Frame {
    Frame::fill(value, 8, 4, PixelFormat::RGB24).unwrap()
}

/// Run `work` on its own thread; fail the test instead of hanging.
fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    within_for(LIMIT, work)
}

fn within_for<T: Send + 'static>(limit: Duration, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, result) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(work());
    });
    result
        .recv_timeout(limit)
        .expect("capture made no progress")
}

fn eventually(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition not reached");
        thread::sleep(Duration::from_millis(1));
    }
}

/// The native capture's side of the seam: the test drives its sink and
/// decides how it returns. Dropping it ends capture cleanly.
struct Native {
    sink: Arc<Mutex<Option<Arc<dyn CaptureSink>>>>,
    exit: mpsc::Sender<CaptureResult>,
}

impl Native {
    fn sink(&self) -> Arc<dyn CaptureSink> {
        eventually(|| self.sink.lock().unwrap().is_some());
        self.sink.lock().unwrap().clone().unwrap()
    }

    fn ready(&self) {
        self.sink().ready();
    }

    fn frame(&self, value: u8) {
        self.sink().frame(frame(value));
    }

    fn stop_requested(&self) -> bool {
        self.sink().should_stop(Duration::ZERO)
    }

    fn wait_stopped(&self) {
        eventually(|| self.stop_requested());
    }

    /// Capture returns `result`.
    fn exit(&self, result: CaptureResult) {
        self.exit.send(result).unwrap();
    }

    fn released(&self) {
        self.exit(Ok(()));
    }
}

fn native() -> (Capture, Native) {
    let sink = Arc::new(Mutex::new(None));
    let (exit, exited) = mpsc::channel();
    let capture: Capture = Box::new({
        let sink = sink.clone();
        move |given: &Arc<dyn CaptureSink>| {
            *sink.lock().unwrap() = Some(given.clone());
            exited.recv().unwrap_or(Ok(()))
        }
    });
    (capture, Native { sink, exit })
}

fn disconnected() -> CaptureError {
    CaptureError::Disconnected {
        details: "cable unplugged".into(),
    }
}

fn start(capture: Capture, deadlines: Deadlines) -> mpsc::Receiver<Result<DeviceFrames>> {
    let (done, started) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(DeviceFrames::start(capture, deadlines));
    });
    started
}

/// An opened source plus the native capture driving it.
fn opened() -> (DeviceFrames, Native) {
    let (capture, native) = native();
    let started = start(capture, TEST);
    native.ready();
    (started.recv_timeout(LIMIT).unwrap().unwrap(), native)
}

fn close(frames: DeviceFrames) -> mpsc::Receiver<Result<()>> {
    let (done, closed) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(Box::new(frames).close());
    });
    closed
}

/// Close, let the capture stop, and expect certified release.
fn closes_cleanly(frames: DeviceFrames, native: &Native) {
    let closed = close(frames);
    native.wait_stopped();
    native.released();
    closed.recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn open_waits_for_readiness_not_frames() {
    let (capture, native) = native();
    let opened = start(capture, TEST);
    assert!(opened.recv_timeout(SHORT).is_err(), "open returned early");
    native.ready();
    let mut frames = opened.recv_timeout(LIMIT).unwrap().unwrap();
    assert!(frames.next_frame(Duration::ZERO).unwrap().is_idle());
    closes_cleanly(frames, &native);
}

#[test]
fn open_failure_reports_the_native_cause_after_release() {
    let (capture, native) = native();
    let opening = start(capture, TEST);
    native.exit(Err(disconnected()));
    let Err(error) = opening.recv_timeout(LIMIT).unwrap() else {
        panic!("open must fail");
    };
    assert_eq!(
        chain(&error),
        "video device capture failed: capture device disconnected: cable unplugged"
    );
}

#[test]
fn open_timeout_stops_and_releases() {
    let (capture, native) = native();
    let deadlines = Deadlines {
        ready: SHORT,
        release: LIMIT,
    };
    let opening = start(capture, deadlines);
    native.wait_stopped();
    native.released();
    let Err(error) = opening.recv_timeout(LIMIT).unwrap() else {
        panic!("open must time out");
    };
    assert!(error.to_string().contains("not ready within"), "{error}");
    assert!(error.source().is_none(), "release was certified");
}

#[test]
fn open_timeout_reports_unconfirmed_release() {
    let (capture, native) = native();
    let deadlines = Deadlines {
        ready: SHORT,
        release: SHORT,
    };
    let Err(error) = within(move || DeviceFrames::start(capture, deadlines)) else {
        panic!("open must time out");
    };
    assert!(native.stop_requested());
    let text = chain(&error);
    assert!(text.contains("not ready within"), "{text}");
    assert!(text.contains("release is unconfirmed"), "{text}");
}

#[test]
fn newest_frame_wins() {
    // Frames before readiness are already replaced.
    let (capture, native) = native();
    let opening = start(capture, TEST);
    for value in 0..100 {
        native.frame(value);
    }
    native.ready();
    let mut frames = opening.recv_timeout(LIMIT).unwrap().unwrap();
    assert_eq!(
        frames
            .next_frame(LIMIT)
            .unwrap()
            .into_frame()
            .unwrap()
            .as_slice(),
        &[99; 96]
    );
    assert!(frames.next_frame(Duration::ZERO).unwrap().is_idle());

    // An untaken frame is replaced, never queued.
    native.frame(1);
    assert_eq!(frames.pending_frame().unwrap().as_slice(), &[1; 96]);
    native.frame(2);
    let newest = frames.next_frame(LIMIT).unwrap().into_frame().unwrap();
    assert_eq!(newest.as_slice(), &[2; 96]);
    assert!(frames.next_frame(Duration::ZERO).unwrap().is_idle());

    closes_cleanly(frames, &native);
}

#[test]
fn timeout_returns_none() {
    let (mut frames, native) = opened();
    let started = Instant::now();
    assert!(frames.next_frame(SHORT).unwrap().is_idle());
    assert!(started.elapsed() >= SHORT);
    closes_cleanly(frames, &native);
}

#[test]
fn native_failure_propagates_and_release_is_still_certified() {
    let (mut frames, native) = opened();
    native.frame(1);
    native.exit(Err(disconnected()));
    // A published failure also stops the sink.
    native.wait_stopped();
    // The failure fences the frame that was never taken.
    let error = frames.next_frame(Duration::ZERO).unwrap_err();
    assert_eq!(
        chain(&error),
        "video device capture failed: capture device disconnected: cable unplugged"
    );
    assert!(
        frames.next_frame(Duration::ZERO).is_err(),
        "failure is sticky"
    );
    close(frames).recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn unexpected_clean_exit_is_a_failure() {
    let (mut frames, native) = opened();
    native.released();
    let error = frames.next_frame(LIMIT).unwrap_err();
    assert!(error.to_string().contains("ended unexpectedly"), "{error}");
    close(frames).recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn close_waits_for_and_certifies_release() {
    let (frames, native) = opened();
    let closed = close(frames);
    native.wait_stopped();
    assert!(
        closed.recv_timeout(SHORT).is_err(),
        "close returned before release"
    );
    native.released();
    closed.recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn close_reports_failed_release() {
    let (frames, native) = opened();
    let closed = close(frames);
    native.wait_stopped();
    native.exit(Err(CaptureError::Cleanup(anyhow::anyhow!(
        "buffer release failed"
    ))));
    let error = closed.recv_timeout(LIMIT).unwrap().unwrap_err();
    assert_eq!(
        chain(&error),
        "video device release is unconfirmed: native capture cleanup failed: buffer release failed: buffer release failed"
    );
}

#[test]
fn close_reports_a_capture_that_does_not_stop() {
    let (capture, native) = native();
    let deadlines = Deadlines {
        ready: LIMIT,
        release: SHORT,
    };
    let opening = start(capture, deadlines);
    native.ready();
    let frames = opening.recv_timeout(LIMIT).unwrap().unwrap();
    let error = close(frames).recv_timeout(LIMIT).unwrap().unwrap_err();
    assert!(native.stop_requested());
    assert!(
        error
            .to_string()
            .contains("video device capture thread did not stop"),
        "{error}"
    );
}

#[test]
fn drop_without_close_stops_and_waits() {
    let (frames, native) = opened();
    let (done, dropped) = mpsc::channel();
    thread::spawn(move || {
        drop(frames);
        let _ = done.send(());
    });
    native.wait_stopped();
    native.released();
    dropped.recv_timeout(LIMIT).unwrap();
}

#[test]
fn settings_file_shape_and_defaults() {
    let settings: Settings =
        serde_json::from_str(r#"{ "capture": { "kind": "device", "path": "/dev/video0" } }"#)
            .unwrap();
    let mut registry = Registry::default();
    registry.add_source(DeviceSource);
    registry.validate(&settings).unwrap();

    let typed: DeviceSettings =
        serde_json::from_value(serde_json::to_value(settings.capture.unwrap().fields).unwrap())
            .unwrap();
    assert_eq!(typed, DeviceSettings::new("/dev/video0"));
    assert_eq!((typed.width, typed.height, typed.fps), (640, 360, 30));

    let encoded = AdapterSettings::new(DeviceSource::KIND, &typed);
    assert_eq!(
        serde_json::to_value(&encoded).unwrap(),
        serde_json::json!({ "kind": "device", "path": "/dev/video0", "width": 640, "height": 360, "fps": 30 })
    );
}

#[test]
fn settings_validation_names_the_field() {
    let field = |change: fn(&mut DeviceSettings)| {
        let mut settings = DeviceSettings::new("/dev/video0");
        change(&mut settings);
        DeviceSource.validate(&settings).unwrap_err().field
    };
    assert_eq!(field(|s| s.path = " ".into()), "path");
    assert_eq!(field(|s| s.path = "a\0b".into()), "path");
    assert_eq!(field(|s| s.width = 0), "width");
    assert_eq!(field(|s| s.width = 3841), "width");
    assert_eq!(field(|s| s.height = 0), "height");
    assert_eq!(field(|s| s.height = 2161), "height");
    assert_eq!(field(|s| s.fps = 0), "fps");

    let mut registry = Registry::default();
    registry.add_source(DeviceSource);
    let unknown: Settings = serde_json::from_str(
        r#"{ "capture": { "kind": "device", "path": "/dev/video0", "rate": 30 } }"#,
    )
    .unwrap();
    assert!(matches!(
        registry.validate(&unknown),
        Err(Error::Decode { .. })
    ));
}

#[test]
fn real_backend_missing_device_fails_without_hanging() {
    let settings = DeviceSettings::new("/dev/prysm-test-missing-device");
    // A backend that does not fail at once is bounded by the adapter's own
    // deadlines: waiting for activation, then for release.
    let Deadlines { ready, release } = Deadlines::DEVICE;
    let Err(error) = within_for(ready + release + LIMIT, move || {
        DeviceSource.open(&settings)
    }) else {
        panic!("a missing device must not open");
    };
    assert!(!chain(&error).is_empty());
}

/// Whatever this machine has (often nothing): each found device fills a
/// `path` that validates, and listing opens nothing. The platform's own
/// enumeration can take seconds (macOS with a Continuity camera).
#[test]
fn candidates_fill_a_valid_path() {
    let found = within_for(Duration::from_secs(30), || {
        let mut registry = Registry::default();
        registry.add_source(DeviceSource);
        registry.candidates(prysm_ports::Section::Capture, "device")
    })
    .unwrap();
    for candidate in found {
        let settings: DeviceSettings =
            serde_json::from_value(candidate.fields.clone().into()).unwrap();
        assert_eq!(settings.validate(), Ok(()), "{candidate:?}");
        assert_eq!(candidate.detail.as_deref(), Some(settings.path.as_str()));
    }
}
