use super::*;
use crate::source::{Deadlines, DeviceFrames, Native};
use futures::FutureExt;
use prysm_capture::{CaptureError, CaptureExit};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{AdapterSettings, Error, Registry, Settings};
use std::error::Error as _;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

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
    let (done, result) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(work());
    });
    result
        .recv_timeout(LIMIT)
        .expect("capture made no progress")
}

fn eventually(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition not reached");
        thread::sleep(Duration::from_millis(1));
    }
}

/// The native worker's side of the seam.
struct Worker {
    ready: Option<oneshot::Sender<()>>,
    frames: watch::Sender<Option<Frame>>,
    cancel: CancellationToken,
    exit: oneshot::Sender<Result<CaptureExit, CaptureError>>,
}

impl Worker {
    fn ready(&mut self) {
        self.ready.take().unwrap().send(()).unwrap();
    }

    /// The worker returns: frames and readiness close, then the join resolves.
    fn exit(self, joined: Result<CaptureExit, CaptureError>) {
        let Self { exit, .. } = self;
        let _ = exit.send(joined);
    }

    fn released(self) {
        self.exit(Ok(CaptureExit { failure: None }));
    }

    fn wait_cancelled(&self) {
        eventually(|| self.cancel.is_cancelled());
    }
}

fn native() -> (Native, Worker) {
    let (ready, ready_rx) = oneshot::channel();
    let (frames, frames_rx) = watch::channel(None);
    let (exit, exit_rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    let native = Native {
        ready: ready_rx,
        frames: frames_rx,
        cancel: cancel.clone(),
        join: async move { exit_rx.await.expect("worker exit was reported") }.boxed(),
    };
    let worker = Worker {
        ready: Some(ready),
        frames,
        cancel,
        exit,
    };
    (native, worker)
}

fn disconnected() -> CaptureError {
    CaptureError::Disconnected {
        details: "cable unplugged".into(),
    }
}

fn chain(error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// An opened source plus the worker driving it.
fn opened() -> (DeviceFrames, Worker) {
    let (native, mut worker) = native();
    worker.ready();
    let frames = within(move || DeviceFrames::start(native, TEST)).unwrap();
    (frames, worker)
}

fn close(frames: DeviceFrames) -> mpsc::Receiver<Result<()>> {
    let (done, closed) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(Box::new(frames).close());
    });
    closed
}

#[test]
fn open_waits_for_readiness_not_frames() {
    let (native, mut worker) = native();
    let (done, opened) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(DeviceFrames::start(native, TEST));
    });
    assert!(opened.recv_timeout(SHORT).is_err(), "open returned early");
    worker.ready();
    let mut frames = opened.recv_timeout(LIMIT).unwrap().unwrap();
    assert!(frames.next_frame(Duration::ZERO).unwrap().is_none());
    let closed = close(frames);
    worker.wait_cancelled();
    worker.released();
    closed.recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn open_failure_reports_the_native_cause_after_release() {
    let (native, worker) = native();
    let opening = within(move || {
        let (done, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = done.send(DeviceFrames::start(native, TEST));
        });
        result
    });
    worker.exit(Ok(CaptureExit {
        failure: Some(disconnected()),
    }));
    let Err(error) = opening.recv_timeout(LIMIT).unwrap() else {
        panic!("open must fail");
    };
    assert_eq!(
        chain(&error),
        "video device capture failed: capture device disconnected: cable unplugged"
    );
}

#[test]
fn open_timeout_cancels_and_releases() {
    let (native, worker) = native();
    let deadlines = Deadlines {
        ready: SHORT,
        release: LIMIT,
    };
    let (done, opening) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(DeviceFrames::start(native, deadlines));
    });
    worker.wait_cancelled();
    worker.released();
    let Err(error) = opening.recv_timeout(LIMIT).unwrap() else {
        panic!("open must time out");
    };
    assert!(error.to_string().contains("not ready within"), "{error}");
    assert!(error.source().is_none(), "release was certified");
}

#[test]
fn open_timeout_reports_unconfirmed_release() {
    let (native, worker) = native();
    let deadlines = Deadlines {
        ready: SHORT,
        release: SHORT,
    };
    let Err(error) = within(move || DeviceFrames::start(native, deadlines)) else {
        panic!("open must time out");
    };
    assert!(worker.cancel.is_cancelled());
    let text = chain(&error);
    assert!(text.contains("not ready within"), "{text}");
    assert!(text.contains("release is unconfirmed"), "{text}");
}

#[test]
fn newest_frame_wins() {
    // Frames before readiness are already replaced natively.
    let (native, mut worker) = native();
    for value in 0..100 {
        worker.frames.send_replace(Some(frame(value)));
    }
    worker.ready();
    let mut frames = within(move || DeviceFrames::start(native, TEST)).unwrap();
    assert_eq!(
        frames.next_frame(LIMIT).unwrap().unwrap().as_slice(),
        &[99; 96]
    );
    assert!(frames.next_frame(Duration::ZERO).unwrap().is_none());

    // An untaken frame is replaced, never queued.
    let pending = |frames: &DeviceFrames, value: u8| {
        frames
            .pending_frame()
            .is_some_and(|frame| frame.as_slice() == [value; 96])
    };
    worker.frames.send_replace(Some(frame(1)));
    eventually(|| pending(&frames, 1));
    worker.frames.send_replace(Some(frame(2)));
    eventually(|| pending(&frames, 2));
    let newest = frames.next_frame(LIMIT).unwrap().unwrap();
    assert_eq!(newest.as_slice(), &[2; 96]);
    assert!(frames.next_frame(Duration::ZERO).unwrap().is_none());

    let closed = close(frames);
    worker.wait_cancelled();
    worker.released();
    closed.recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn timeout_returns_none() {
    let (mut frames, worker) = opened();
    let started = Instant::now();
    assert!(frames.next_frame(SHORT).unwrap().is_none());
    assert!(started.elapsed() >= SHORT);
    let closed = close(frames);
    worker.wait_cancelled();
    worker.released();
    closed.recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn native_failure_propagates_and_release_is_still_certified() {
    let (mut frames, worker) = opened();
    worker.frames.send_replace(Some(frame(1)));
    eventually(|| frames.pending_frame().is_some());
    worker.exit(Ok(CaptureExit {
        failure: Some(disconnected()),
    }));
    eventually(|| frames.exited());
    // The exit fences the frame that was never taken.
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
    let (mut frames, worker) = opened();
    worker.released();
    let error = frames.next_frame(LIMIT).unwrap_err();
    assert!(error.to_string().contains("ended unexpectedly"), "{error}");
    close(frames).recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn close_waits_for_and_certifies_release() {
    let (frames, worker) = opened();
    let closed = close(frames);
    worker.wait_cancelled();
    assert!(
        closed.recv_timeout(SHORT).is_err(),
        "close returned before release"
    );
    worker.released();
    closed.recv_timeout(LIMIT).unwrap().unwrap();
}

#[test]
fn close_reports_failed_release() {
    let (frames, worker) = opened();
    let closed = close(frames);
    worker.wait_cancelled();
    worker.exit(Err(CaptureError::Cleanup(anyhow::anyhow!(
        "buffer release failed"
    ))));
    let error = closed.recv_timeout(LIMIT).unwrap().unwrap_err();
    assert_eq!(
        chain(&error),
        "video device release is unconfirmed: native capture cleanup failed: buffer release failed: buffer release failed"
    );
}

#[test]
fn close_reports_a_worker_that_does_not_stop() {
    let (native, mut worker) = native();
    worker.ready();
    let deadlines = Deadlines {
        ready: LIMIT,
        release: SHORT,
    };
    let frames = within(move || DeviceFrames::start(native, deadlines)).unwrap();
    let error = close(frames).recv_timeout(LIMIT).unwrap().unwrap_err();
    assert!(worker.cancel.is_cancelled());
    assert!(
        error
            .to_string()
            .contains("native capture worker did not stop"),
        "{error}"
    );
}

#[test]
fn drop_without_close_cancels() {
    let (frames, worker) = opened();
    drop(frames);
    assert!(worker.cancel.is_cancelled());
    worker.released();
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
    assert_eq!(field(|s| s.path = " ".into()), "capture.path");
    assert_eq!(field(|s| s.path = "a\0b".into()), "capture.path");
    assert_eq!(field(|s| s.width = 0), "capture.width");
    assert_eq!(field(|s| s.width = 3841), "capture.width");
    assert_eq!(field(|s| s.height = 0), "capture.height");
    assert_eq!(field(|s| s.height = 2161), "capture.height");
    assert_eq!(field(|s| s.fps = 0), "capture.fps");

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
    let Err(error) = within(move || DeviceSource.open(&settings)) else {
        panic!("a missing device must not open");
    };
    assert!(!chain(&error).is_empty());
}
