//! Development-host tests through the native seam: no SDK, no hardware, every
//! wait bounded.
use super::*;
use crate::classify::{Reason, Recovery, UiRecovery};
use crate::source::{Acquired, Backend, Fault, Limits, Tap, UiFault, UiTap};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{AdapterSettings, Error, Pull, Registry, Settings, chain};
use prysm_testkit::{eventually, within};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(5);
const SHORT: Duration = Duration::from_millis(50);
/// Short enough for tests, long enough that only a deliberate hang trips it.
const TEST: Limits = Limits {
    watchdog: Duration::from_secs(2),
    preparation: Duration::from_secs(2),
    pending: Duration::from_secs(2),
    not_ready_retry: Duration::from_millis(5),
    release: Duration::from_secs(2),
};

/// A video fault that fails the source, or one retried with a new driver.
fn recovery(fatal: bool) -> Recovery {
    if fatal {
        Recovery::Escalate(prysm_ports::Escalation::FAIL_FAST)
    } else {
        Recovery::Retry(Reason::FreshDriver)
    }
}

/// A TV UI fault that cannot heal, or one that may pass.
fn ui_recovery(fatal: bool) -> UiRecovery {
    if fatal {
        UiRecovery::Unsupported
    } else {
        UiRecovery::MayPass
    }
}

fn frame(value: u8) -> Frame {
    Frame::fill(value, 8, 4, PixelFormat::RGB24).unwrap()
}

/// The fake UI tap's frames: a different size from video's, as on the TV.
fn ui_frame(value: u8) -> Frame {
    Frame::fill(value, 4, 2, PixelFormat::RGB24).unwrap()
}

// ---- Scripted driver ----------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Step {
    Frame(u8),
    Pending,
    NotReady,
    Refused,
    Fail {
        fatal: bool,
    },
    /// The TV withholds the video (`vtCapture_planeInfo` 13).
    Withheld,
}

struct Script {
    /// Consumed one per acquisition; then `idle` repeats.
    steps: VecDeque<Step>,
    idle: Step,
    open_fails: Option<Recovery>,
    hang: Option<&'static str>,
    release_fails: bool,
    opens: usize,
    releases: usize,
    acquires: usize,
    /// The video tap's driver is created and not initialized, as the real
    /// one is after its open or a refused or not-ready initialization.
    created: bool,
    /// The UI tap fails to open (fatally or not), fails every acquisition,
    /// or fails once.
    ui_open_fails: Option<bool>,
    ui_fails: bool,
    ui_fail_once: bool,
    ui_value: u8,
    ui_opens: usize,
    ui_releases: usize,
    /// Taps open right now, and the most ever open together.
    open_taps: usize,
    most_open: usize,
    /// Native calls other than video acquisitions, with their thread.
    events: Vec<(&'static str, ThreadId)>,
}

impl Script {
    fn record(&mut self, event: &'static str) {
        self.events.push((event, thread::current().id()));
    }

    fn opened(&mut self) {
        self.open_taps += 1;
        self.most_open = self.most_open.max(self.open_taps);
    }
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
                created: false,
                ui_open_fails: None,
                ui_fails: false,
                ui_fail_once: false,
                ui_value: 200,
                ui_opens: 0,
                ui_releases: 0,
                open_taps: 0,
                most_open: 0,
                events: Vec::new(),
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

    /// Recorded native calls, repeated UI acquisitions collapsed; all must
    /// have run on one thread, which is not the test's.
    fn events(&self) -> Vec<&'static str> {
        let script = self.script();
        let threads: Vec<_> = script.events.iter().map(|(_, thread)| *thread).collect();
        assert!(
            threads.windows(2).all(|pair| pair[0] == pair[1]),
            "threads differ"
        );
        assert!(!threads.contains(&thread::current().id()));
        let mut events: Vec<_> = script.events.iter().map(|(event, _)| *event).collect();
        events.dedup();
        events
    }
}

impl Backend for Driver {
    fn open(&mut self, _: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        let mut script = self.enter("open");
        script.opens += 1;
        script.record("open");
        if let Some(recovery) = script.open_fails {
            return Err(Fault {
                error: Error::other("driver refused"),
                recovery,
            });
        }
        script.opened();
        script.created = true;
        Ok(Box::new(FakeTap(self.clone())))
    }

    fn open_ui(&mut self, settings: &WebosSettings) -> Result<Box<dyn UiTap>, UiFault> {
        assert_eq!(settings.ui_size(), (320, 180));
        let mut script = self.enter("UI open");
        script.ui_opens += 1;
        script.record("UI open");
        if let Some(fatal) = script.ui_open_fails {
            return Err(UiFault {
                error: Error::adapter(
                    "cannot open webOS UI capture",
                    std::io::Error::other("HAL_GAL_Init failed with status -1"),
                ),
                recovery: ui_recovery(fatal),
            });
        }
        script.opened();
        Ok(Box::new(FakeUiTap(self.clone())))
    }
}

struct FakeUiTap(Driver);

impl UiTap for FakeUiTap {
    fn acquire(&mut self) -> Result<Frame, UiFault> {
        let mut script = self.0.enter("UI acquire");
        script.record("UI acquire");
        if script.ui_fails || std::mem::take(&mut script.ui_fail_once) {
            return Err(UiFault {
                error: Error::other("HAL_GAL_CaptureFrameBuffer failed with status -1"),
                recovery: UiRecovery::MayPass,
            });
        }
        Ok(ui_frame(script.ui_value))
    }

    fn release(self: Box<Self>) -> prysm_ports::Result<()> {
        let mut script = self.0.enter("UI release");
        script.ui_releases += 1;
        script.open_taps -= 1;
        script.record("UI release");
        Ok(())
    }
}

struct FakeTap(Driver);

impl Tap for FakeTap {
    fn acquire(&mut self) -> Result<Acquired, Fault> {
        let mut script = self.0.enter("acquire");
        script.acquires += 1;
        let step = script.steps.pop_front().unwrap_or(script.idle);
        script.created = matches!(step, Step::NotReady | Step::Refused);
        let status = |status: &str, recovery| {
            Err(Fault {
                error: Error::adapter("webOS video interrupted", std::io::Error::other(status)),
                recovery,
            })
        };
        match step {
            Step::Frame(value) => Ok(Acquired::Frame(frame(value))),
            Step::Pending => Ok(Acquired::Pending),
            Step::NotReady => Ok(Acquired::NotReady),
            Step::Refused => Ok(Acquired::Refused),
            Step::Fail { fatal } => status("status 5", recovery(fatal)),
            Step::Withheld => status("status 13", Recovery::Wait),
        }
    }

    fn created_only(&self) -> bool {
        self.0.script().created
    }

    fn release(self: Box<Self>) -> prysm_ports::Result<()> {
        let mut script = self.0.enter("release");
        script.releases += 1;
        script.open_taps -= 1;
        script.record("release");
        if script.release_fails {
            return Err(Error::other("vtCapture_stop failed with status 5"));
        }
        Ok(())
    }
}

/// Video-only settings: the video tap's lifecycle without the UI tap.
fn start_with(driver: &Driver, video: &VideoSignal, limits: Limits) -> WebosFrames {
    let settings = WebosSettings {
        fps: 60,
        include_ui: false,
        ..WebosSettings::default()
    };
    WebosFrames::start(driver.clone(), settings, video.clone(), limits).unwrap()
}

/// Default settings (`include_ui` on), fast.
fn start_ui(driver: &Driver, video: &VideoSignal) -> WebosFrames {
    start_ui_with(driver, video, TEST)
}

fn start_ui_with(driver: &Driver, video: &VideoSignal, limits: Limits) -> WebosFrames {
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

/// Wait for the next frame that matches, failing the test after [`LIMIT`].
fn next_where(frames: &mut WebosFrames, matches: impl Fn(&Frame) -> bool) -> Frame {
    let deadline = Instant::now() + LIMIT;
    loop {
        let frame = next(frames);
        if matches(&frame) {
            return frame;
        }
        assert!(Instant::now() < deadline, "no matching frame");
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
    let mut frames = within(LIMIT, {
        let (driver, video) = (driver.clone(), video.clone());
        move || start(&driver, &video)
    });
    assert!(matches!(frames.next_frame(SHORT * 2).unwrap(), Pull::Quiet));
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
    eventually("`driver.count(|s| s.releases) == 1`", LIMIT, || {
        (driver.count(|s| s.releases) == 1).then_some(())
    });
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Quiet));
    assert_eq!(driver.count(|s| s.opens), 1);

    driver.push([Step::Frame(2)]);
    video.set(Video::Available);
    assert_eq!(next(&mut frames).as_slice(), &[2; 96]);
    assert_eq!(driver.count(|s| s.opens), 2);

    // A context change restarts the tap without losing the source.
    video.reset();
    eventually("`driver.count(|s| s.releases) == 2`", LIMIT, || {
        (driver.count(|s| s.releases) == 2).then_some(())
    });
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
    eventually("`driver.count(|s| s.acquires) > 3`", LIMIT, || {
        (driver.count(|s| s.acquires) > 3).then_some(())
    });
    assert_eq!(frames.pending_frame().unwrap().as_slice(), &[3; 96]);
    assert_eq!(next(&mut frames).as_slice(), &[3; 96]);
    assert!(matches!(
        frames.next_frame(Duration::ZERO).unwrap(),
        Pull::Quiet
    ));
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

/// The driver's opens, acquisitions and releases so far.
fn calls(driver: &Driver) -> (usize, usize, usize) {
    let script = driver.script();
    (script.opens, script.acquires, script.releases)
}

/// Pull until one says refused, failing the test after [`LIMIT`].
fn refused(frames: &mut WebosFrames) {
    let deadline = Instant::now() + LIMIT;
    while !matches!(frames.next_frame(SHORT).unwrap(), Pull::Refused) {
        assert!(Instant::now() < deadline, "no refusal");
    }
}

/// The video signal with available video, and a source on it whose driver
/// refuses its first initialization, then answers `then`.
fn refusing(then: Step) -> (Driver, VideoSignal, WebosFrames) {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Available);
    driver.push([Step::Refused]);
    driver.with(|s| s.idle = then);
    let frames = start(&driver, &video);
    eventually("the refusal", LIMIT, || {
        (driver.count(|s| s.acquires) == 1).then_some(())
    });
    (driver, video, frames)
}

/// A refused initialization is no failure and runs no timer: past the
/// not-ready retry and preparation limits the pulls say refused, the one driver is
/// neither initialized again nor released.
#[test]
fn a_refusal_waits_for_the_video_signal_on_the_same_driver() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Refused]);
    let limits = Limits {
        preparation: Duration::from_millis(50),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    refused(&mut frames);
    assert!(matches!(
        frames.next_frame(SHORT * 4).unwrap(),
        Pull::Refused
    ));
    assert_eq!(calls(&driver), (1, 1, 0));
    closes_cleanly(frames);
}

/// A new context initializes the refused driver once more: refused again,
/// it waits again; accepted, frames come and the pulls stop saying refused.
/// The driver is never released or opened again.
#[test]
fn a_new_context_initializes_the_refused_driver_once_more() {
    let (driver, video, mut frames) = refusing(Step::Refused);
    refused(&mut frames);
    video.reset();
    eventually("one more initialization", LIMIT, || {
        (driver.count(|s| s.acquires) == 2).then_some(())
    });
    thread::sleep(SHORT);
    assert_eq!(calls(&driver), (1, 2, 0), "exactly one");
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Refused));

    driver.with(|s| s.idle = Step::Pending);
    driver.push([Step::Frame(4)]);
    video.reset();
    assert_eq!(next(&mut frames).as_slice(), &[4; 96]);
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Quiet));
    assert_eq!((calls(&driver).0, calls(&driver).2), (1, 0), "same driver");
    closes_cleanly(frames);
}

/// Changes during an initialization are seen after it even when they leave
/// the video as it was: the refused driver is initialized once more.
#[test]
fn changes_during_an_initialization_are_not_lost() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Available);
    driver.push([Step::Refused, Step::Frame(8)]);
    driver.hang("acquire");
    let mut frames = start(&driver, &video);
    eventually("the open", LIMIT, || {
        (driver.count(|s| s.opens) == 1).then_some(())
    });
    video.set(Video::Unavailable);
    video.set(Video::Available);
    driver.unhang();
    assert_eq!(next(&mut frames).as_slice(), &[8; 96]);
    assert_eq!((calls(&driver).0, calls(&driver).2), (1, 0));
    closes_cleanly(frames);
}

/// A change that leaves the video as it was releases and reopens nothing
/// while a tap runs.
#[test]
fn an_unchanged_video_leaves_a_running_tap_alone() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Available);
    driver.push([Step::Frame(1)]);
    let mut frames = start(&driver, &video);
    assert_eq!(next(&mut frames).as_slice(), &[1; 96]);
    video.set(Video::Available);
    driver.push([Step::Frame(2)]);
    assert_eq!(next(&mut frames).as_slice(), &[2; 96]);
    assert_eq!((calls(&driver).0, calls(&driver).2), (1, 0));
    closes_cleanly(frames);
}

/// A not-ready answer after a refusal leaves the pulls refused: only a
/// frame ends that, and then they stop saying refused. The driver is
/// neither released nor opened again.
#[test]
fn a_refusal_outlives_a_not_ready_answer() {
    let (driver, video, mut frames) = refusing(Step::NotReady);
    refused(&mut frames);
    video.reset();
    eventually("the initialization after the change", LIMIT, || {
        (driver.count(|s| s.acquires) >= 2).then_some(())
    });
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Refused));
    driver.with(|s| s.idle = Step::Pending);
    driver.push([Step::Frame(7)]);
    assert_eq!(next(&mut frames).as_slice(), &[7; 96]);
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Quiet));
    assert_eq!((calls(&driver).0, calls(&driver).2), (1, 0));
    closes_cleanly(frames);
}

/// Alternating pending and not-ready answers never stall, however short
/// the pending limit: each pending run counts from its own first answer.
/// They reach the preparation limit, counted from the first answer without
/// a frame, which fails the source on its one driver.
#[test]
fn alternating_not_ready_and_pending_answers_reach_the_preparation_limit() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Available);
    driver.push([Step::Pending, Step::NotReady].repeat(1000));
    driver.with(|s| s.idle = Step::NotReady);
    let limits = Limits {
        pending: Duration::from_millis(1),
        preparation: Duration::from_millis(300),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    let error = failure(&mut frames);
    assert!(error.to_string().contains("not ready within"), "{error}");
    closes_cleanly(frames);
    let (opens, _, releases) = calls(&driver);
    assert_eq!((opens, releases), (1, 1), "no stall opened a new driver");
}

/// A driver that answered not ready is created only too: a new context
/// initializes it again instead of releasing it.
#[test]
fn a_context_reset_keeps_a_created_only_driver() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Available);
    driver.push([Step::NotReady]);
    let limits = Limits {
        not_ready_retry: Duration::from_secs(60),
        preparation: Duration::from_secs(120),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    eventually("not ready", LIMIT, || {
        (driver.count(|s| s.acquires) == 1).then_some(())
    });
    driver.push([Step::Frame(5)]);
    video.reset();
    assert_eq!(next(&mut frames).as_slice(), &[5; 96]);
    // The thread goes on acquiring at its pace after the frame.
    let (opens, acquires, releases) = calls(&driver);
    assert_eq!((opens, releases), (1, 0));
    assert!(acquires >= 2, "{acquires} acquisitions");

    // Refused, then a new context: the same driver again.
    driver.push([Step::Refused]);
    video.reset();
    refused(&mut frames);
    assert_eq!(calls(&driver).2, 1, "the running tap was released");
    driver.push([Step::Frame(6)]);
    video.reset();
    assert_eq!(next(&mut frames).as_slice(), &[6; 96]);
    assert_eq!((calls(&driver).0, calls(&driver).2), (2, 1));
    closes_cleanly(frames);
}

/// Video that is gone releases a refused driver, and the pulls go quiet.
#[test]
fn unavailable_video_releases_a_refused_driver() {
    let (driver, video, mut frames) = refusing(Step::Refused);
    refused(&mut frames);
    video.set(Video::Unavailable);
    eventually("the release", LIMIT, || {
        (calls(&driver).2 == 1).then_some(())
    });
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Quiet));
    closes_cleanly(frames);
    assert_eq!(calls(&driver), (1, 1, 1));
}

/// Stopping the capture releases a refused driver.
#[test]
fn stopping_releases_a_refused_driver() {
    let (driver, _video, mut frames) = refusing(Step::Refused);
    refused(&mut frames);
    closes_cleanly(frames);
    assert_eq!(calls(&driver), (1, 1, 1));
}

/// A stall opens one new driver at once, held here in its open so the
/// next frame is queued for it.
#[test]
fn stalled_producer_restarts_the_tap() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.with(|s| s.idle = Step::Frame(1));
    let limits = Limits {
        pending: Duration::from_millis(50),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    assert_eq!(next(&mut frames).as_slice(), &[1; 96]);
    driver.hang("open");
    driver.with(|s| s.idle = Step::Pending);
    eventually("the stall's release", LIMIT, || {
        (calls(&driver).2 == 1).then_some(())
    });
    driver.push([Step::Frame(2)]);
    driver.unhang();
    // Queued after the release: only the new driver can deliver it.
    next_where(&mut frames, |f| f.as_slice()[0] == 2);
    closes_cleanly(frames);
}

/// The new driver stalling too, with no frame between, is the TV
/// withholding the picture: no failure, no timer; the pulls say refused
/// until a change of the video signal opens a new driver and frames come.
#[test]
fn two_stalls_in_a_row_wait_for_the_video_signal() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Available);
    let limits = Limits {
        pending: Duration::from_millis(50),
        ..TEST
    };
    let mut frames = start_with(&driver, &video, limits);
    refused(&mut frames);
    let (opens, _, releases) = calls(&driver);
    assert_eq!((opens, releases), (2, 2), "one new driver, both released");
    assert_eq!(driver.count(|s| s.open_taps), 0);
    driver.push([Step::Frame(3)]);
    video.reset();
    // Queued with no tap held: only a third driver can deliver it.
    assert_eq!(next(&mut frames).as_slice(), &[3; 96]);
    closes_cleanly(frames);
}

/// No video plane to capture (status 13) on the first acquisition: the tap
/// is released and nothing is opened until the video signal changes; the
/// pulls say refused meanwhile. A change opens a new tap and frames come.
#[test]
fn a_withheld_video_waits_for_the_video_signal() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Available);
    driver.push([Step::Withheld]);
    let mut frames = start(&driver, &video);
    refused(&mut frames);
    let (opens, acquires, releases) = calls(&driver);
    assert_eq!((opens, acquires, releases), (1, 1, 1), "idle, no timer");
    driver.push([Step::Frame(4)]);
    video.reset();
    assert_eq!(next(&mut frames).as_slice(), &[4; 96]);
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Quiet));
    let (opens, _, releases) = calls(&driver);
    assert_eq!((opens, releases), (2, 1));
    closes_cleanly(frames);
    assert_eq!(calls(&driver).2, 2);
}

/// An ordinary failure opens one new driver at once, without an error; a
/// frame gives that try back, so a second failure after it does the same.
/// Two failures with no frame between (here two failed opens after a
/// context change) fail the source.
#[test]
fn an_ordinary_failure_opens_one_new_driver() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    let fail = Step::Fail { fatal: false };
    driver.push([Step::Frame(1), fail, Step::Frame(2), fail, Step::Frame(3)]);
    let mut frames = start(&driver, &video);
    next_where(&mut frames, |f| f.as_slice()[0] == 3);
    let (opens, _, releases) = calls(&driver);
    assert_eq!((opens, releases), (3, 2));

    driver.with(|s| s.open_fails = Some(recovery(false)));
    video.reset();
    assert_eq!(failure(&mut frames).to_string(), "driver refused");
    closes_cleanly(frames);
    let (opens, _, releases) = calls(&driver);
    assert_eq!((opens, releases), (5, 3), "one more open, then the failure");
}

/// A tap's open only creates its driver, so a failed initialization is
/// its first acquisition's fault: the tap is released once and reopened.
#[test]
fn a_failed_first_acquisition_releases_and_reopens_the_tap() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Fail { fatal: false }, Step::Frame(3)]);
    let mut frames = start(&driver, &video);
    assert_eq!(next(&mut frames).as_slice(), &[3; 96]);
    // The thread goes on acquiring at its pace after the frame.
    let (opens, acquires, releases) = calls(&driver);
    assert_eq!((opens, releases), (2, 1));
    assert!(acquires >= 2, "{acquires} acquisitions");
    closes_cleanly(frames);
}

#[test]
fn fatal_failure_propagates_with_its_cause_and_release_is_certified() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1), Step::Fail { fatal: true }]);
    let mut frames = start(&driver, &video);
    let error = failure(&mut frames);
    assert_eq!(chain(&error), "webOS video interrupted: status 5");
    assert!(
        frames.next_frame(Duration::ZERO).is_err(),
        "failure is sticky"
    );
    eventually("`driver.count(|s| s.releases) == 1`", LIMIT, || {
        (driver.count(|s| s.releases) == 1).then_some(())
    });
    closes_cleanly(frames);
}

#[test]
fn fatal_open_failure_propagates() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.with(|s| s.open_fails = Some(recovery(true)));
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
            .contains("native acquire in the webOS capture thread exceeded its"),
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
    eventually("`driver.count(|s| s.releases) == 1`", LIMIT, || {
        (driver.count(|s| s.releases) == 1).then_some(())
    });
}

// ---- TV UI without video -------------------------------------------------

#[test]
fn ui_tap_replaces_video_only_while_there_is_none() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let mut frames = start_ui(&driver, &video);
    assert_eq!(next(&mut frames).as_slice(), &[1; 96]);
    assert_eq!(driver.count(|s| s.ui_opens), 0, "UI tap next to video");

    // Home: video is released before the UI tap opens; its frames flow,
    // at their own size.
    video.set(Video::Unavailable);
    let ui = next_where(&mut frames, |f| f.width() == 4);
    assert_eq!(
        (ui.width(), ui.height(), ui.as_slice()),
        (4, 2, &[200; 24][..])
    );
    driver.with(|s| s.ui_value = 201);
    assert_eq!(
        next_where(&mut frames, |f| f.as_slice()[0] == 201).width(),
        4
    );

    // Back to an input: UI released, video reopened.
    driver.push([Step::Frame(2)]);
    video.set(Video::Available);
    assert_eq!(
        next_where(&mut frames, |f| f.width() == 8).as_slice(),
        &[2; 96]
    );
    // A context change on Home reopens the UI tap.
    let captured = |opens| {
        let script = driver.script();
        script.ui_opens == opens && script.events.last().unwrap().0 == "UI acquire"
    };
    video.set(Video::Unavailable);
    eventually("`captured(2)`", LIMIT, || (captured(2)).then_some(()));
    video.reset();
    eventually("`captured(3)`", LIMIT, || (captured(3)).then_some(()));
    closes_cleanly(frames);

    assert_eq!(driver.count(|s| s.most_open), 1, "two taps were open");
    assert_eq!(
        driver.events(),
        [
            "open",
            "release",
            "UI open",
            "UI acquire",
            "UI release",
            "open",
            "release",
            "UI open",
            "UI acquire",
            "UI release",
            "UI open",
            "UI acquire",
            "UI release",
        ]
    );
}

#[test]
fn source_can_start_on_home() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Unavailable);
    let mut frames = start_ui(&driver, &video);
    assert_eq!(next(&mut frames).as_slice(), &[200; 24]);
    assert_eq!(driver.count(|s| s.opens), 0);
    closes_cleanly(frames);
    assert_eq!(driver.events(), ["UI open", "UI acquire", "UI release"]);
}

#[test]
fn without_include_ui_there_are_no_frames_without_video() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Unavailable);
    let mut frames = start(&driver, &video);
    thread::sleep(SHORT);
    assert!(matches!(frames.next_frame(SHORT).unwrap(), Pull::Quiet));
    assert_eq!(driver.count(|s| s.ui_opens), 0);
    closes_cleanly(frames);
}

/// A UI open that cannot heal: the UI tap is not opened again in this
/// source, and no failure ends it. Video comes and goes as before, and its
/// loss opens no UI tap.
#[test]
fn a_ui_failure_that_cannot_heal_makes_ui_capture_unsupported() {
    {
        let driver = Driver::new();
        let video = VideoSignal::default();
        video.set(Video::Unavailable);
        driver.with(|s| s.ui_open_fails = Some(true));
        let mut frames = start_ui(&driver, &video);
        eventually("the UI tap to fail", LIMIT, || {
            let script = driver.script();
            (script.ui_opens == 1 && script.open_taps == 0).then_some(())
        });
        driver.with(|s| s.ui_open_fails = None);
        for value in [5, 6] {
            driver.push([Step::Frame(value)]);
            video.set(Video::Available);
            assert_eq!(next(&mut frames).as_slice(), &[value; 96]);
            video.set(Video::Unavailable);
            eventually("the video tap's release", LIMIT, || {
                (driver.count(|s| s.open_taps) == 0).then_some(())
            });
        }
        assert!(
            matches!(frames.next_frame(SHORT).unwrap(), Pull::Quiet),
            "no failure"
        );
        closes_cleanly(frames);
        let opened: Vec<_> = driver
            .events()
            .into_iter()
            .filter(|event| event.ends_with("open"))
            .collect();
        assert_eq!(opened, ["UI open", "open", "open"]);
    }
}

/// A UI open or acquire failure that may pass stops the tries, without a
/// failure, until the video signal changes: a new context, or video that
/// comes back and goes again, opens the UI tap again.
#[test]
fn a_ui_failure_that_may_pass_waits_for_the_video_signal() {
    for (open_fails, acquire_fails) in [(Some(false), false), (None, true)] {
        let driver = Driver::new();
        let video = VideoSignal::default();
        video.set(Video::Unavailable);
        driver.with(|s| {
            s.ui_open_fails = open_fails;
            s.ui_fails = acquire_fails;
        });
        let mut frames = start_ui(&driver, &video);
        let failed = |opens| {
            let script = driver.script();
            (script.ui_opens == opens && script.open_taps == 0).then_some(())
        };
        eventually("the UI tap to fail", LIMIT, || failed(1));
        video.reset();
        eventually("the UI tap to fail again", LIMIT, || failed(2));
        driver.with(|s| {
            s.ui_open_fails = None;
            s.ui_fails = false;
        });
        driver.push([Step::Frame(5)]);
        video.set(Video::Available);
        assert_eq!(next(&mut frames).as_slice(), &[5; 96]);
        video.set(Video::Unavailable);
        assert_eq!(next_where(&mut frames, |f| f.width() == 4).width(), 4);
        closes_cleanly(frames);
        let mut opened: Vec<_> = driver
            .events()
            .into_iter()
            .filter(|event| event.ends_with("open"))
            .collect();
        // The two failed UI opens as one, as `events` collapses them when
        // nothing came between.
        opened.dedup();
        assert_eq!(opened, ["UI open", "open", "UI open"], "{open_fails:?}");
        assert_eq!(driver.count(|s| s.ui_opens), 3, "{open_fails:?}");
    }
}

#[test]
fn only_a_missing_ui_library_or_device_is_permanent() {
    use prysm_capture_webos_sys as sys;
    let device = |kind| sys::Error::Device {
        path: "/dev/gfx",
        source: std::io::Error::from(kind),
    };
    let fatal = |fault: UiFault| fault.recovery == UiRecovery::Unsupported;
    assert!(fatal(backend::ui_open_fault(device(
        std::io::ErrorKind::NotFound
    ))));
    assert!(!fatal(backend::ui_open_fault(device(
        std::io::ErrorKind::OutOfMemory
    ))));
    let status = sys::Error::Status {
        operation: sys::Operation::HalInit,
        code: -1,
    };
    let fault = backend::ui_open_fault(status);
    assert_eq!(fault.recovery, UiRecovery::MayPass);
    assert_eq!(
        chain(&fault.error),
        "cannot open webOS UI capture: HAL_GAL_Init failed with status -1"
    );
    if std::path::Path::new("/dev/gfx").exists() {
        return; // On a TV this would open real hardware; not a host test.
    }
    let load = sys::UiCapture::open(320, 180).err().unwrap();
    assert!(matches!(load, sys::Error::Load { .. }), "{load}");
    assert!(fatal(backend::ui_open_fault(load)));
}

/// Only a failed driver creation or a name left registered needs a new
/// process; every other failure leaves the host free to open a new source.
#[test]
fn only_a_failed_create_or_a_stuck_name_needs_a_new_process() {
    use prysm_capture_webos_sys as sys;
    use prysm_webos_luna_sys::Error as Luna;
    let status = || sys::Error::Status {
        operation: sys::Operation::VtInit,
        code: 11,
    };
    let message = || "failed".to_owned();
    for (error, restart) in [
        (sys::Error::Create(Box::new(status())), true),
        (
            sys::Error::from(Luna::Unremoved {
                name: "a".into(),
                message: message(),
            }),
            true,
        ),
        (
            sys::Error::from(Luna::Denied {
                name: "a".into(),
                message: message(),
            }),
            false,
        ),
        (
            sys::Error::from(Luna::Luna {
                operation: "LSRegister",
                message: message(),
            }),
            false,
        ),
        (status(), false),
    ] {
        let failure = backend::fault("cannot open webOS video capture", error).error;
        assert_eq!(needs_new_process(&failure), restart, "{}", chain(&failure));
    }
    assert!(!needs_new_process(&Error::other("anything else")));
}

/// Any failed Luna registration check ends video capture at once, whatever
/// LS2 answered; the cause still says which failure it was.
#[test]
fn every_failed_registration_check_escalates_and_names_its_cause() {
    use prysm_capture_webos_sys as sys;
    use prysm_webos_luna_sys::Error as Luna;
    let open = |error| backend::fault("cannot open webOS video capture", error);
    let message = || "failed".to_owned();
    for (error, cause) in [
        (
            Luna::Luna {
                operation: "LSRegister",
                message: message(),
            },
            "the capture registration could not be checked",
        ),
        (
            Luna::Denied {
                name: "a".into(),
                message: message(),
            },
            "the capture registration is not allowed",
        ),
        (
            Luna::Unremoved {
                name: "a".into(),
                message: message(),
            },
            "a capture client name stays registered",
        ),
    ] {
        let fault = open(sys::Error::from(error));
        assert!(
            matches!(fault.recovery, Recovery::Escalate(_)),
            "{}",
            chain(&fault.error)
        );
        assert!(
            chain(&fault.error).starts_with(&format!("cannot open webOS video capture: {cause}")),
            "{}",
            chain(&fault.error)
        );
    }
    assert_eq!(
        open(sys::Error::Status {
            operation: sys::Operation::VtProcess,
            code: 1,
        })
        .recovery,
        Recovery::Retry(Reason::FreshDriver)
    );
}

/// A UI tap that worked and then fails is released and not reopened while
/// the video signal stays as it is.
#[test]
fn a_ui_fault_after_it_worked_waits_without_an_error() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    video.set(Video::Unavailable);
    let mut frames = start_ui(&driver, &video);
    next(&mut frames);
    driver.with(|s| s.ui_fail_once = true);
    eventually("`driver.count(|s| s.ui_releases) == 1`", LIMIT, || {
        (driver.count(|s| s.ui_releases) == 1).then_some(())
    });
    driver.push([Step::Frame(7)]);
    video.set(Video::Available);
    assert_eq!(next(&mut frames).as_slice(), &[7; 96]);
    closes_cleanly(frames);
    assert_eq!(driver.count(|s| s.ui_opens), 1);
}

#[test]
fn ui_size_is_capped_and_floored() {
    let size = |width, height| {
        WebosSettings {
            width,
            height,
            ..WebosSettings::default()
        }
        .ui_size()
    };
    assert_eq!(size(640, 360), (320, 180), "default is capped");
    assert_eq!(size(1920, 1080), (320, 180));
    assert_eq!(size(320, 180), (320, 180));
    assert_eq!(size(288, 162), (288, 162), "smaller sizes are kept");
    assert_eq!(size(256, 144), (256, 144));
    assert_eq!(size(160, 90), (256, 144), "HAL fails below 256x144");
    assert_eq!(size(2, 2), (256, 144));
    // Non-16:9: each axis on its own; the plane is stretched to fill.
    assert_eq!(size(640, 480), (320, 180));
    assert_eq!(size(200, 1080), (256, 180));
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

/// A release that fails mid-run (here at a change of context) fails the
/// source with its cause, and `close` reports the release unconfirmed.
#[test]
fn a_failed_release_mid_run_leaves_the_release_unconfirmed() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let mut frames = start(&driver, &video);
    next(&mut frames);
    driver.with(|s| s.release_fails = true);
    video.reset();
    let unconfirmed = "webOS video release is unconfirmed: vtCapture_stop failed with status 5";
    assert_eq!(chain(&failure(&mut frames)), unconfirmed);
    let error = close(frames).recv_timeout(LIMIT).unwrap().unwrap_err();
    assert_eq!(chain(&error), unconfirmed);
    assert_eq!(
        (calls(&driver).0, calls(&driver).2),
        (1, 1),
        "released once, not reopened"
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
    eventually("`driver.count(|s| s.releases) == 1`", LIMIT, || {
        (driver.count(|s| s.releases) == 1).then_some(())
    });
}

#[test]
fn drop_without_close_still_releases() {
    let driver = Driver::new();
    let video = VideoSignal::default();
    driver.push([Step::Frame(1)]);
    let mut frames = start(&driver, &video);
    next(&mut frames);
    drop(frames);
    eventually("`driver.count(|s| s.releases) == 1`", LIMIT, || {
        (driver.count(|s| s.releases) == 1).then_some(())
    });
}

/// Records its name when dropped.
struct Dropped(&'static str, Arc<Mutex<Vec<&'static str>>>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.1.lock().unwrap().push(self.0);
    }
}

/// Opens taps that panic on their first acquisition.
struct PanickingBackend(Dropped);

impl Backend for PanickingBackend {
    fn open(&mut self, _: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        Ok(Box::new(PanickingTap {
            _dropped: Dropped("tap", self.0.1.clone()),
        }))
    }

    fn open_ui(&mut self, _: &WebosSettings) -> Result<Box<dyn UiTap>, UiFault> {
        unreachable!("video-only settings")
    }
}

/// Records its drop, nothing else.
struct PanickingTap {
    _dropped: Dropped,
}

impl Tap for PanickingTap {
    fn acquire(&mut self) -> Result<Acquired, Fault> {
        panic!("native bug");
    }

    fn created_only(&self) -> bool {
        true
    }

    fn release(self: Box<Self>) -> prysm_ports::Result<()> {
        Ok(())
    }
}

#[test]
fn a_panic_drops_the_tap_while_the_backend_still_holds_the_device() {
    let drops = Arc::new(Mutex::new(Vec::new()));
    let backend = PanickingBackend(Dropped("backend", drops.clone()));
    let settings = WebosSettings {
        include_ui: false,
        ..WebosSettings::default()
    };
    // The panic leaves `acquire` a watched call until the thread has caught
    // it, after the panic hook has printed it (seconds under a sanitizer).
    // So the watchdog cannot fire first, and the wait is long.
    let limits = Limits {
        watchdog: Duration::MAX,
        ..TEST
    };
    let mut frames = WebosFrames::start(backend, settings, VideoSignal::default(), limits).unwrap();
    let failure = frames.next_frame(Duration::from_secs(60));
    assert!(matches!(failure, Err(Error::Panicked(_))), "{failure:?}");
    assert!(close(frames).recv_timeout(LIMIT).unwrap().is_err());
    assert_eq!(*drops.lock().unwrap(), ["tap", "backend"]);
}

#[test]
fn production_limits_fit_the_runtime_deadlines() {
    let tv = Limits::TV;
    assert_eq!(tv.watchdog, Duration::from_secs(5));
    assert_eq!(tv.preparation, Duration::from_secs(10));
    // The TV hosts assert their stop deadlines against it.
    assert_eq!(tv.release, RELEASE);
}

// ---- Real backend on the development host -------------------------------

#[test]
fn real_backend_refuses_a_non_tv_host_without_hanging() {
    if cfg!(all(target_os = "linux", target_arch = "arm")) {
        return; // On a TV this would touch hardware; not a host test.
    }
    let Err(error) = within(LIMIT, || {
        WebosCapture::default().start(&WebosSettings::default())
    }) else {
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
    assert_eq!(field(|s| s.width = 0), "width");
    assert_eq!(field(|s| s.width = 641), "width");
    assert_eq!(field(|s| s.width = 1922), "width");
    assert_eq!(field(|s| s.height = 0), "height");
    assert_eq!(field(|s| s.height = 361), "height");
    assert_eq!(field(|s| s.height = 1082), "height");
    assert_eq!(field(|s| s.fps = 0), "fps");
    assert_eq!(field(|s| s.fps = 61), "fps");
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
