//! Development-host tests through the native seam: no SDK, no hardware, every
//! wait bounded.
use super::*;
use crate::source::{Acquired, Backend, Fault, Limits, Tap};
use prysm_core::{Frame, FramePool, PixelFormat};
use prysm_ports::{AdapterSettings, Error, Registry, Settings};
use std::collections::VecDeque;
use std::error::Error as _;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(5);
const SHORT: Duration = Duration::from_millis(50);
/// Short enough for tests, long enough that only a deliberate hang trips it.
const TEST: Limits = Limits {
    watchdog: Duration::from_secs(2),
    preparation: Duration::from_secs(2),
    pending: Duration::from_secs(2),
    retry: Duration::from_millis(5),
    release: Duration::from_secs(2),
    poll: Duration::from_millis(5),
};

fn frame(value: u8) -> Frame {
    Frame::fill(value, 8, 4, PixelFormat::RGB24).unwrap()
}

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

// ---- Scripted driver ----------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Step {
    Frame(u8),
    Pending,
    NotReady,
    Fail { fatal: bool },
}

struct Script {
    /// Consumed one per acquisition; then `idle` repeats.
    steps: VecDeque<Step>,
    idle: Step,
    open_fails: Option<bool>,
    hang: Option<&'static str>,
    release_fails: bool,
    opens: usize,
    releases: usize,
    acquires: usize,
}

/// The test's handle on the native driver.
#[derive(Clone)]
struct Driver(Arc<(Mutex<Script>, Condvar)>);

impl Driver {
    fn new() -> Self {
        Self(Arc::new((
            Mutex::new(Script {
                steps: VecDeque::new(),
                idle: Step::Pending,
                open_fails: None,
                hang: None,
                release_fails: false,
                opens: 0,
                releases: 0,
                acquires: 0,
            }),
            Condvar::new(),
        )))
    }

    fn script(&self) -> MutexGuard<'_, Script> {
        self.0.0.lock().unwrap()
    }

    fn with(&self, change: impl FnOnce(&mut Script)) {
        change(&mut self.script());
        self.0.1.notify_all();
    }

    fn push(&self, steps: impl IntoIterator<Item = Step>) {
        self.with(|script| script.steps.extend(steps));
    }

    /// Block in `operation` until [`Driver::unhang`].
    fn hang(&self, operation: &'static str) {
        self.with(|script| script.hang = Some(operation));
    }

    fn unhang(&self) {
        self.with(|script| script.hang = None);
    }

    fn enter(&self, operation: &'static str) -> MutexGuard<'_, Script> {
        let (lock, changed) = &*self.0;
        changed
            .wait_while(lock.lock().unwrap(), |script| {
                script.hang == Some(operation)
            })
            .unwrap()
    }

    fn count(&self, read: impl Fn(&Script) -> usize) -> usize {
        read(&self.script())
    }
}

impl Backend for Driver {
    fn open(&mut self, _: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        let mut script = self.enter("open");
        script.opens += 1;
        if let Some(fatal) = script.open_fails {
            return Err(Fault {
                error: Error::other("driver refused"),
                fatal,
            });
        }
        Ok(Box::new(FakeTap(self.clone())))
    }
}

struct FakeTap(Driver);

impl Tap for FakeTap {
    fn acquire(&mut self) -> Result<Acquired, Fault> {
        let mut script = self.0.enter("acquire");
        script.acquires += 1;
        let step = script.steps.pop_front().unwrap_or(script.idle);
        match step {
            Step::Frame(value) => Ok(Acquired::Frame(frame(value))),
            Step::Pending => Ok(Acquired::Pending),
            Step::NotReady => Ok(Acquired::NotReady),
            Step::Fail { fatal } => Err(Fault {
                error: Error::adapter(
                    "webOS video interrupted",
                    std::io::Error::other("status 12"),
                ),
                fatal,
            }),
        }
    }

    fn release(self: Box<Self>) -> prysm_ports::Result<()> {
        let mut script = self.0.enter("release");
        script.releases += 1;
        if script.release_fails {
            return Err(Error::other("vtCapture_stop failed with status 5"));
        }
        Ok(())
    }
}

fn start_with(driver: &Driver, video: &VideoSignal, limits: Limits) -> WebosFrames {
    let settings = WebosSettings {
        fps: 60,
        ..WebosSettings::default()
    };
    WebosFrames::start(driver.clone(), settings, video.clone(), limits).unwrap()
}

fn start(driver: &Driver, video: &VideoSignal) -> WebosFrames {
    start_with(driver, video, TEST)
}

fn close(frames: WebosFrames) -> mpsc::Receiver<prysm_ports::Result<()>> {
    let (done, closed) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(Box::new(frames).close());
    });
    closed
}

/// Wait for the next frame, failing the test after [`LIMIT`].
fn next(frames: &mut WebosFrames) -> Frame {
    let deadline = Instant::now() + LIMIT;
    loop {
        if let Some(frame) = frames.next_frame(SHORT).unwrap().into_frame() {
            return frame;
        }
        assert!(Instant::now() < deadline, "no frame");
    }
}

/// Wait for a terminal error, failing the test after [`LIMIT`].
fn failure(frames: &mut WebosFrames) -> Error {
    let deadline = Instant::now() + LIMIT;
    loop {
        match frames.next_frame(SHORT) {
            Err(error) => return error,
            Ok(_) => assert!(Instant::now() < deadline, "no failure"),
        }
    }
}

fn closes_cleanly(frames: WebosFrames) {
    close(frames).recv_timeout(LIMIT).unwrap().unwrap();
}

// ---- Lifecycle -----------------------------------------------------------

#[test]
fn waits_for_video_before_opening_the_tap() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Unavailable);
    let mut frames = within({
        let (driver, video) = (driver.clone(), video.clone());
        move || start(&driver, &video)
    });
    assert!(frames.next_frame(SHORT * 2).unwrap().is_idle());
    assert_eq!(driver.count(|s| s.opens), 0, "opened without video");

    driver.push([Step::Frame(7)]);
    video.set(Video::Available);
    assert_eq!(next(&mut frames).as_slice(), &[7; 96]);
    assert_eq!(driver.count(|s| s.opens), 1);
    closes_cleanly(frames);
    assert_eq!(driver.count(|s| s.releases), 1);
}

#[test]
fn losing_video_releases_the_tap_and_frames_resume() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let mut frames = start(&driver, &video);
    assert_eq!(next(&mut frames).as_slice(), &[1; 96]);

    video.set(Video::Unavailable);
    eventually(|| driver.count(|s| s.releases) == 1);
    assert!(frames.next_frame(SHORT).unwrap().is_idle());
    assert_eq!(driver.count(|s| s.opens), 1);

    driver.push([Step::Frame(2)]);
    video.set(Video::Available);
    assert_eq!(next(&mut frames).as_slice(), &[2; 96]);
    assert_eq!(driver.count(|s| s.opens), 2);

    // A context change restarts the tap without losing the source.
    video.reset();
    eventually(|| driver.count(|s| s.releases) == 2);
    driver.push([Step::Frame(3)]);
    assert_eq!(next(&mut frames).as_slice(), &[3; 96]);
    assert_eq!(driver.count(|s| s.opens), 3);
    closes_cleanly(frames);
}

#[test]
fn newest_frame_wins() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1), Step::Frame(2), Step::Frame(3)]);
    let mut frames = start(&driver, &video);
    // All three are acquired before anything is taken; only the last remains.
    eventually(|| driver.count(|s| s.acquires) > 3);
    assert_eq!(frames.pending_frame().unwrap().as_slice(), &[3; 96]);
    assert_eq!(next(&mut frames).as_slice(), &[3; 96]);
    assert!(frames.next_frame(Duration::ZERO).unwrap().is_idle());
    closes_cleanly(frames);
}

#[test]
fn preparation_retries_then_frames_flow() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([
        Step::NotReady,
        Step::NotReady,
        Step::Pending,
        Step::Frame(4),
    ]);
    let mut frames = start(&driver, &video);
    assert_eq!(next(&mut frames).as_slice(), &[4; 96]);
    assert_eq!(driver.count(|s| s.opens), 1, "preparation reuses the tap");
    closes_cleanly(frames);
}

#[test]
fn preparation_limit_is_a_failure_and_release_is_still_certified() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.with(|s| s.idle = Step::NotReady);
    let limits = Limits {
        preparation: Duration::from_millis(100),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    let error = failure(&mut frames);
    assert!(error.to_string().contains("not ready within"), "{error}");
    closes_cleanly(frames);
    assert_eq!(driver.count(|s| s.releases), 1);
}

#[test]
fn stalled_producer_restarts_the_tap() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let limits = Limits {
        pending: Duration::from_millis(50),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    assert_eq!(next(&mut frames).as_slice(), &[1; 96]);
    eventually(|| driver.count(|s| s.releases) >= 1 && driver.count(|s| s.opens) >= 2);
    driver.push([Step::Frame(2)]);
    assert_eq!(next(&mut frames).as_slice(), &[2; 96]);
    closes_cleanly(frames);
}

#[test]
fn recoverable_failure_reopens_without_an_error() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1), Step::Fail { fatal: false }, Step::Frame(2)]);
    let mut frames = start(&driver, &video);
    assert_eq!(next(&mut frames).as_slice(), &[1; 96]);
    assert_eq!(next(&mut frames).as_slice(), &[2; 96]);
    assert_eq!(driver.count(|s| s.opens), 2);
    assert_eq!(driver.count(|s| s.releases), 1);

    driver.with(|s| s.open_fails = Some(false));
    video.reset();
    eventually(|| driver.count(|s| s.opens) >= 4);
    assert!(
        frames.next_frame(SHORT).unwrap().is_idle(),
        "open retries quietly"
    );
    closes_cleanly(frames);
}

#[test]
fn fatal_failure_propagates_with_its_cause_and_release_is_certified() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1), Step::Fail { fatal: true }]);
    let mut frames = start(&driver, &video);
    let error = failure(&mut frames);
    assert_eq!(chain(&error), "webOS video interrupted: status 12");
    assert!(
        frames.next_frame(Duration::ZERO).is_err(),
        "failure is sticky"
    );
    eventually(|| driver.count(|s| s.releases) == 1);
    closes_cleanly(frames);
}

#[test]
fn fatal_open_failure_propagates() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.with(|s| s.open_fails = Some(true));
    let mut frames = start(&driver, &video);
    assert_eq!(failure(&mut frames).to_string(), "driver refused");
    closes_cleanly(frames);
    assert_eq!(driver.count(|s| s.releases), 0, "nothing was opened");
}

#[test]
fn watchdog_reports_a_hung_native_call() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.hang("acquire");
    let limits = Limits {
        watchdog: Duration::from_millis(100),
        release: Duration::from_millis(100),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    let error = failure(&mut frames);
    assert!(
        error
            .to_string()
            .contains("native webOS acquire exceeded its"),
        "{error}"
    );
    let error = close(frames).recv_timeout(LIMIT).unwrap().unwrap_err();
    assert!(
        error.to_string().contains("release is unconfirmed"),
        "{error}"
    );
    assert!(
        error.to_string().contains("native acquire has run"),
        "{error}"
    );
    // The driver returns late; the thread still releases the tap.
    driver.unhang();
    eventually(|| driver.count(|s| s.releases) == 1);
}

// ---- Close and drop ------------------------------------------------------

#[test]
fn close_waits_for_and_certifies_release() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let mut frames = start(&driver, &video);
    next(&mut frames);
    driver.hang("release");
    let closed = close(frames);
    assert!(
        closed.recv_timeout(SHORT * 2).is_err(),
        "close returned before release"
    );
    driver.unhang();
    closed.recv_timeout(LIMIT).unwrap().unwrap();
    assert_eq!(driver.count(|s| s.releases), 1);
}

#[test]
fn close_reports_failed_release() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let mut frames = start(&driver, &video);
    next(&mut frames);
    driver.with(|s| s.release_fails = true);
    let error = close(frames).recv_timeout(LIMIT).unwrap().unwrap_err();
    assert_eq!(
        chain(&error),
        "webOS video release is unconfirmed: vtCapture_stop failed with status 5"
    );
}

#[test]
fn close_reports_unconfirmed_release_within_its_bound() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let limits = Limits {
        release: Duration::from_millis(100),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    next(&mut frames);
    driver.hang("release");
    let started = Instant::now();
    let error = close(frames).recv_timeout(LIMIT).unwrap().unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        error.to_string().contains("release is unconfirmed"),
        "{error}"
    );
    assert!(
        error.to_string().contains("native release has run"),
        "{error}"
    );
    driver.unhang();
    eventually(|| driver.count(|s| s.releases) == 1);
}

#[test]
fn drop_without_close_still_releases() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let mut frames = start(&driver, &video);
    next(&mut frames);
    drop(frames);
    eventually(|| driver.count(|s| s.releases) == 1);
}

#[test]
fn production_limits_fit_the_runtime_deadlines() {
    let tv = Limits::TV;
    assert_eq!(tv.watchdog, Duration::from_secs(5));
    assert_eq!(tv.preparation, Duration::from_secs(10));
    // Below the forwarder's 5 s and the engine's 10 s stop deadlines.
    assert!(tv.release < Duration::from_secs(5));
}

// ---- Real backend on the development host -------------------------------

#[test]
fn real_backend_refuses_a_non_tv_host_without_hanging() {
    if cfg!(all(target_os = "linux", target_arch = "arm")) {
        return; // On a TV this would touch hardware; not a host test.
    }
    let Err(error) = within(|| WebosCapture::default().start(&WebosSettings::default())) else {
        panic!("a development host cannot capture TV video");
    };
    assert!(error.to_string().contains("LG webOS TV"), "{error}");
}

#[test]
fn capture_lock_is_exclusive_until_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("capture.lock");
    let lock = backend::capture_lock(&path).unwrap();
    let error = backend::capture_lock(&path).unwrap_err();
    assert!(
        error.to_string().contains("another Prysm capturer"),
        "{error}"
    );
    drop(lock);
    backend::capture_lock(&path).unwrap();

    let link = directory.path().join("link.lock");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(backend::capture_lock(&link).is_err(), "followed a symlink");
}

#[test]
fn nv12_limited_range_becomes_full_range_rgb() {
    let mut pool = FramePool::default();
    let frame = backend::nv12_to_rgb(&mut pool, 2, 2, &[16, 235, 235, 16, 128, 128]).unwrap();
    assert_eq!(
        frame.as_slice(),
        [0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0]
    );
    assert!(backend::nv12_to_rgb(&mut pool, 2, 2, &[16; 5]).is_err());
}

// ---- Settings ------------------------------------------------------------

#[test]
fn settings_file_shape_and_defaults() {
    let settings: Settings = serde_json::from_str(r#"{ "capture": { "kind": "webos" } }"#).unwrap();
    let mut registry = Registry::default();
    registry.add_source(WebosCapture::default());
    registry.validate(&settings).unwrap();

    let typed: WebosSettings =
        serde_json::from_value(serde_json::to_value(settings.capture.unwrap().fields).unwrap())
            .unwrap();
    assert_eq!(typed, WebosSettings::default());

    let encoded = AdapterSettings::new(WebosCapture::KIND, &typed);
    assert_eq!(
        serde_json::to_value(&encoded).unwrap(),
        serde_json::json!({ "kind": "webos", "width": 640, "height": 360, "fps": 30, "include_ui": true })
    );
}

#[test]
fn settings_validation_names_the_field() {
    let field = |change: fn(&mut WebosSettings)| {
        let mut settings = WebosSettings::default();
        change(&mut settings);
        WebosCapture::default()
            .validate(&settings)
            .unwrap_err()
            .field
    };
    assert_eq!(field(|s| s.width = 0), "capture.width");
    assert_eq!(field(|s| s.width = 641), "capture.width");
    assert_eq!(field(|s| s.width = 1922), "capture.width");
    assert_eq!(field(|s| s.height = 0), "capture.height");
    assert_eq!(field(|s| s.height = 361), "capture.height");
    assert_eq!(field(|s| s.height = 1082), "capture.height");
    assert_eq!(field(|s| s.fps = 0), "capture.fps");
    assert_eq!(field(|s| s.fps = 61), "capture.fps");
    let full_hd = WebosSettings {
        width: 1920,
        height: 1080,
        fps: 60,
        include_ui: false,
    };
    WebosCapture::default().validate(&full_hd).unwrap();

    let mut registry = Registry::default();
    registry.add_source(WebosCapture::default());
    let unknown: Settings =
        serde_json::from_str(r#"{ "capture": { "kind": "webos", "rate": 30 } }"#).unwrap();
    assert!(matches!(
        registry.validate(&unknown),
        Err(Error::Decode { .. })
    ));
}
