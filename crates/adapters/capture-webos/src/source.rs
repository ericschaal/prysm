//! One opened webOS source. Its [`OwnerThread`] owns the backend and its tap
//! (LG requires load, capture and release on one thread). It follows the
//! [`VideoSignal`] to pick the tap kind (video, or the TV UI plane when there
//! is no video and `include_ui` is on; never both), paces acquisition,
//! retries recoverable interruptions and publishes the newest frame, a
//! terminal failure and the release result. The session thread only waits
//! with a deadline and enforces the native-operation watchdog, so a hung
//! driver call surfaces as an error instead of a hang.
mod native;

use crate::{VideoSignal, WebosSettings};
use native::Native;
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, OwnerThread, Pull, Result};
use std::time::Duration;

/// One acquisition attempt.
pub(crate) enum Acquired {
    Frame(Frame),
    /// No new buffer yet.
    Pending,
    /// The driver is (re)initializing; try again after [`Limits::retry`].
    NotReady,
}

/// A native failure. Recoverable ones release the tap and reopen it.
#[derive(Debug)]
pub(crate) struct Fault {
    pub error: Error,
    pub recovery: Recovery,
}

/// Whether a native fault can heal in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// Retrying in this process cannot succeed: a video fault ends the
    /// source, a TV UI fault disables UI capture for the session.
    Never,
    /// The tap is reopened later.
    Reopen,
    /// The video tap is reopened later, but [`Limits::unchecked`] of these in
    /// a row end the source: the Luna registration check failed in a way it
    /// cannot tell from a denial worded differently.
    Bounded,
}

impl Fault {
    /// A failure that cannot heal in this process.
    pub(crate) fn fatal(error: Error) -> Self {
        Self {
            error,
            recovery: Recovery::Never,
        }
    }

    /// A failure after which the tap is reopened.
    pub(crate) fn recoverable(error: Error) -> Self {
        Self {
            error,
            recovery: Recovery::Reopen,
        }
    }
}

/// An open native tap. Created, used and released on the native thread.
pub(crate) trait Tap {
    fn acquire(&mut self) -> Result<Acquired, Fault>;
    /// `Ok` certifies the driver released the tap.
    fn release(self: Box<Self>) -> Result<()>;
}

/// Opens taps. Moved to the native thread and dropped there after the last
/// tap's release, so anything it holds (the device lock) outlives the tap.
pub(crate) trait Backend: Send + 'static {
    /// The video tap (blended with menus when `include_ui` is on).
    fn open(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault>;
    /// The TV UI (graphics plane) tap, used only while there is no video.
    fn open_ui(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Longest a single native call may run before the source fails.
    pub watchdog: Duration,
    /// Longest the driver may stay [`Acquired::NotReady`].
    pub preparation: Duration,
    /// Longest the driver may stay [`Acquired::Pending`] before the tap is restarted.
    pub pending: Duration,
    /// Delay before reopening after a recoverable failure or between
    /// preparation attempts.
    pub retry: Duration,
    /// Delay before reopening the TV UI tap after its first failure since a
    /// frame; it doubles with each further failure up to `ui_retry_max`,
    /// and a failure after that wait is the last. Slower than `retry`, and
    /// bounded: a failing UI open can block for as long as `release`, so a
    /// close during one is unconfirmed, and a tap that never opens (its
    /// driver lock held until a reboot) would block that long every time.
    pub ui_retry: Duration,
    pub ui_retry_max: Duration,
    /// Video opens in a row with a [`Recovery::Bounded`] fault before the
    /// source fails with the last one: about half a minute at `retry`.
    pub unchecked: u32,
    /// How long `close` waits for certified release. Below the TV hosts'
    /// 6 s stop deadline, so the adapter reports first, which is below the
    /// supervisor's 8 s TERM-to-KILL window.
    pub release: Duration,
    /// How often an idle native thread rechecks the video signal.
    pub poll: Duration,
}

impl Limits {
    pub(crate) const TV: Self = Self {
        watchdog: Duration::from_secs(5),
        preparation: Duration::from_secs(10),
        pending: Duration::from_secs(2),
        retry: Duration::from_secs(1),
        ui_retry: Duration::from_secs(5),
        ui_retry_max: Duration::from_secs(60),
        unchecked: 30,
        release: Duration::from_secs(4),
        poll: Duration::from_millis(100),
    };

    /// The wait after a TV UI failure, given the previous wait of the same
    /// streak of failures (`None`: the first since a frame); `None` once the
    /// longest wait did not help either.
    pub(crate) fn ui_backoff(&self, previous: Option<Duration>) -> Option<Duration> {
        match previous {
            None => Some(self.ui_retry),
            Some(wait) if wait >= self.ui_retry_max => None,
            Some(wait) => Some((wait * 2).min(self.ui_retry_max)),
        }
    }
}

/// The TV's video as a [`FrameSource`]: the newest frame, [`Pull::Idle`]
/// while video is unavailable or the driver is preparing, and `Err` on a terminal
/// native failure or a native call that exceeds its watchdog.
pub struct WebosFrames {
    thread: OwnerThread,
}

impl std::fmt::Debug for WebosFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebosFrames").finish_non_exhaustive()
    }
}

impl WebosFrames {
    /// Start the native thread. It opens the tap once video allows it.
    pub(crate) fn start(
        backend: impl Backend,
        settings: WebosSettings,
        video: VideoSignal,
        limits: Limits,
    ) -> Result<Self> {
        // Taps are not `Send`; they only ever exist on this thread. If it
        // cannot start, nothing native was opened.
        let thread = OwnerThread::spawn(
            "prysm-webos-capture",
            "webOS capture thread",
            limits.release,
            move |worker| Native::new(backend, settings, video, limits, worker.clone()).run(),
        )?
        .with_watchdog(limits.watchdog);
        Ok(Self { thread })
    }

    /// The frame published and not yet taken.
    #[cfg(test)]
    pub(crate) fn pending_frame(&self) -> Option<Frame> {
        self.thread.peek()
    }
}

impl FrameSource for WebosFrames {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        self.thread.next_frame(timeout)
    }

    /// Stop the thread and wait for certified release, within [`Limits::release`].
    fn close(self: Box<Self>) -> Result<()> {
        self.thread.close()
    }
}
