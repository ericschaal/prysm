//! One opened webOS source. Its [`OwnerThread`] owns the backend and its tap
//! (LG requires load, capture and release on one thread). It follows the
//! [`VideoSignal`] to pick the tap kind (video, or the TV UI plane when there
//! is no video and `include_ui` is on; never both), paces acquisition,
//! recovers from faults as [`crate::classify`] says and publishes the newest
//! frame, refusals, a terminal failure and the release result. The session thread only waits
//! with a deadline and enforces the native-operation watchdog, so a hung
//! driver call surfaces as an error instead of a hang.
mod native;
mod phase;

use crate::classify::{self, Recovery, UiRecovery, VideoCause};
use crate::{VideoSignal, WebosSettings};
use native::Native;
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, OwnerThread, Pull, Result};
use std::time::Duration;

/// One acquisition attempt.
#[derive(Debug)]
pub(crate) enum Acquired {
    Frame(Frame),
    /// No new buffer yet.
    Pending,
    /// The driver is (re)initializing; asked again as
    /// [`Limits::preparation`] says.
    NotReady,
    /// The driver refuses to capture this video (`vtCapture_init`
    /// answered 17); its driver stays created, and the next acquisition
    /// initializes it again.
    Refused,
}

/// A native failure, with what its tap does next as [`crate::classify`]
/// says: a video fault's [`Recovery`], or a TV UI fault's [`UiRecovery`].
#[derive(Debug)]
pub(crate) struct Fault<R = Recovery> {
    pub error: Error,
    pub recovery: R,
}

/// A fault of the TV UI tap.
pub(crate) type UiFault = Fault<UiRecovery>;

impl Fault {
    /// A video fault: `source` under `context`, its error escalating as
    /// `recovery` says.
    pub(crate) fn video(
        context: &str,
        source: impl std::error::Error + Send + Sync + 'static,
        recovery: Recovery,
    ) -> Self {
        Self {
            error: Error::adapter_escalating(context, source, recovery.escalation()),
            recovery,
        }
    }

    /// A video fault of a limit the driver reached, which has no native
    /// source: `context` is its error, recovering and escalating as
    /// `cause`'s row says.
    pub(crate) fn limit(context: String, cause: VideoCause<'_>) -> Self {
        let recovery = classify::video(cause);
        Self {
            error: Error::other_escalating(context, recovery.escalation()),
            recovery,
        }
    }
}

/// The open video tap. Created, used and released on the native thread.
pub(crate) trait Tap {
    fn acquire(&mut self) -> Result<Acquired, Fault>;
    /// `Ok` certifies the driver released the tap.
    fn release(self: Box<Self>) -> Result<()>;
}

/// The open TV UI tap. It has no separate initialization, so it answers
/// only a frame or a fault.
pub(crate) trait UiTap {
    fn acquire(&mut self) -> Result<Frame, UiFault>;
    /// `Ok` certifies the driver released the tap.
    fn release(self: Box<Self>) -> Result<()>;
}

/// Opens taps. Moved to the native thread and dropped there after the last
/// tap's release, so anything it holds (the device lock) outlives the tap.
pub(crate) trait Backend: Send + 'static {
    /// The video tap (blended with menus when `include_ui` is on).
    fn open(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault>;
    /// The TV UI (graphics plane) tap, used only while there is no video.
    fn open_ui(&mut self, settings: &WebosSettings) -> Result<Box<dyn UiTap>, UiFault>;
}

/// How long closing a [`WebosFrames`] waits for the driver's certified
/// release before it reports the release unconfirmed. The TV hosts' stop
/// deadlines are set above it, so the adapter reports first.
pub const RELEASE: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Longest a single native call may run before the source fails.
    pub watchdog: Duration,
    /// Longest without a frame, counted from the first answer without one,
    /// at which an [`Acquired::NotReady`] fails the source; until then a
    /// driver not ready is asked again every [`Self::not_ready_retry`].
    /// This converges: the driver answers not ready while the TV's video
    /// pipeline starts, which it finishes within this limit. A change of
    /// the video context restarts that pipeline, so it initializes a driver
    /// not ready again at once and the count starts again from its answer.
    pub preparation: Duration,
    /// Longest without a frame, counted from the first of the
    /// [`Acquired::Pending`] answers since the driver last answered
    /// anything else (it was initialized then), at which one is a stall.
    pub pending: Duration,
    /// Delay between two initializations of a driver that answered
    /// [`Acquired::NotReady`].
    pub not_ready_retry: Duration,
    /// How long `close` waits for certified release ([`RELEASE`]).
    pub release: Duration,
}

impl Limits {
    pub(crate) const TV: Self = Self {
        watchdog: Duration::from_secs(5),
        preparation: Duration::from_secs(10),
        pending: Duration::from_secs(2),
        not_ready_retry: Duration::from_secs(1),
        release: RELEASE,
    };
}

/// The TV's video as a [`FrameSource`]: the newest frame, [`Pull::Quiet`]
/// while video is unavailable or the driver is preparing, [`Pull::Refused`]
/// from the first refused initialization or withholding of the video until
/// a frame or until video is unavailable, and `Err` on a terminal native
/// failure or a native call that exceeds its watchdog.
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
