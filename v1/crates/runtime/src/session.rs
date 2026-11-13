use crate::stats::Stats;
use crate::worker::{Flags, Pumped, Worker};
use prysm_core::{Config, EdgeColors, Frame, Viewport};
use prysm_ports::{
    Error, FrameSource, IdleMode, LatestSlot, LedOutput, OutputIssue, Result, SessionPhase,
};
use prysm_processor::PrysmProcessor;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A quiet source must not let the device time out of live mode: the last
/// colors are rewritten at this interval.
const REFRESH: Duration = Duration::from_millis(500);

/// How long a source may deliver no frames before the session releases the
/// output to [`RELEASE`] (no signal, screen saver, no sender). The output is
/// opened only at the first frame and again at the first frame after a
/// release.
pub(crate) const VIDEO_GRACE: Duration = Duration::from_secs(3);

/// What a released output shows while waiting for frames: the device's own
/// state (WLED's preset or effect).
pub(crate) const RELEASE: IdleMode = IdleMode::Native;

/// Opens the session's output. Called on the session thread at the first
/// frame and at the first frame after each release, and at the end when the
/// final idle mode differs from the one the output was left in.
pub(crate) type OpenOutput = Box<dyn FnMut() -> Result<Box<dyn LedOutput>> + Send>;

/// The latest processed frame, for surfaces that render what the LEDs show.
#[derive(Debug, Clone)]
pub struct Preview {
    pub frame: Frame,
    pub colors: EdgeColors,
    /// The part of `frame` that was sampled, after black-bar removal; the
    /// whole frame when the processor rejected it.
    pub viewport: Viewport,
    /// Increases with every published preview, across sessions, so viewers
    /// can skip a frame they already showed.
    pub sequence: u64,
}

/// Where the session publishes previews. It publishes only while at least
/// one [`PreviewWatch`] exists, so nobody watching costs one atomic load per
/// frame and pins no frame buffer. Empty while no session runs.
#[derive(Debug)]
pub(crate) struct PreviewSlot {
    latest: LatestSlot<Preview>,
    viewers: AtomicUsize,
    sequence: AtomicU64,
}

impl Default for PreviewSlot {
    fn default() -> Self {
        Self {
            latest: LatestSlot::new(),
            viewers: AtomicUsize::new(0),
            sequence: AtomicU64::new(1),
        }
    }
}

impl PreviewSlot {
    /// Register a viewer until the returned watch is dropped.
    #[must_use]
    pub(crate) fn watch(self: &Arc<Self>) -> PreviewWatch {
        self.viewers.fetch_add(1, Ordering::SeqCst);
        PreviewWatch(self.clone())
    }

    /// How many watches exist.
    #[must_use]
    pub(crate) fn viewers(&self) -> usize {
        self.viewers.load(Ordering::SeqCst)
    }

    fn publish(&self, frame: Frame, colors: EdgeColors, viewport: Viewport) {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.latest.offer(Preview {
            frame,
            colors,
            viewport,
            sequence,
        });
        // The last watch may have gone between the check and the offer; its
        // clear could have run first, so do not leave the frame pinned.
        if self.viewers() == 0 {
            self.latest.take();
        }
    }

    fn clear(&self) {
        self.latest.take();
    }
}

/// A registered preview viewer. Cheap to hold; dropping the last one stops
/// publication and releases the held frame.
#[derive(Debug)]
pub struct PreviewWatch(Arc<PreviewSlot>);

impl PreviewWatch {
    /// The latest preview without taking it: other viewers see it too.
    /// `None` while no session runs or before its first frame.
    #[must_use]
    pub fn latest(&self) -> Option<Preview> {
        self.0.latest.peek()
    }
}

impl Drop for PreviewWatch {
    fn drop(&mut self) {
        if self.0.viewers.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.clear();
        }
    }
}

/// How a session ended. Release and idle run on every exit path.
#[derive(Debug)]
pub(crate) struct SessionEnd {
    /// Why the loop stopped; `None` when it was asked to or the source ended.
    pub(crate) failure: Option<Error>,
    /// The source reported [`Pull::Ended`](prysm_ports::Pull::Ended): the
    /// session finished on its own without a failure.
    pub(crate) ended: bool,
    pub(crate) released: Result<()>,
    pub(crate) idled: Result<()>,
}

impl SessionEnd {
    /// The first failure, in the order it happened.
    ///
    /// # Errors
    /// The loop failure, else the release failure, else the idle failure.
    pub(crate) fn result(self) -> Result<()> {
        match self.failure {
            Some(failure) => Err(failure),
            None => self.released.and(self.idled),
        }
    }
}

/// The session's hold on its output.
enum Leds {
    /// Never opened by this session.
    Untouched,
    Held(Box<dyn LedOutput>),
    /// Idled in this mode and closed.
    Left(IdleMode),
    /// A release failed: the device may be in any mode.
    Unknown,
}

impl Leds {
    fn write(&mut self, colors: &EdgeColors, open: &mut OpenOutput, stats: &Stats) -> Result<()> {
        if let Self::Held(output) = self {
            return stats.write(output.as_mut(), colors);
        }
        let mut output = open()?;
        stats.output_opened();
        let written = stats.write(output.as_mut(), colors);
        *self = Self::Held(output);
        written
    }

    /// What a held output last learned about its device.
    fn observe(&mut self) -> Option<OutputIssue> {
        self.held().and_then(LedOutput::observe)
    }

    fn held(&mut self) -> Option<&mut dyn LedOutput> {
        match self {
            Self::Held(output) => Some(output.as_mut()),
            Self::Untouched | Self::Left(_) | Self::Unknown => None,
        }
    }

    /// Idle a held output in `mode` and close it.
    fn release(&mut self, mode: IdleMode) -> Result<()> {
        let Self::Held(output) = std::mem::replace(self, Self::Unknown) else {
            return Ok(());
        };
        output.idle(mode)?;
        *self = Self::Left(mode);
        Ok(())
    }

    /// Leave the output in `mode`: idle it if held; otherwise open it only
    /// when it may be in another mode. An untouched output stays untouched
    /// for `Off`, the state lighting started from.
    fn finish(mut self, mode: IdleMode, open: &mut OpenOutput) -> Result<()> {
        match self {
            Self::Held(_) => self.release(mode),
            Self::Left(left) if left == mode => Ok(()),
            Self::Untouched if mode == IdleMode::Off => Ok(()),
            Self::Untouched | Self::Left(_) | Self::Unknown => open()?.idle(mode),
        }
    }
}

struct Control {
    config: LatestSlot<Config>,
    idle: Mutex<IdleMode>,
    /// The held output's latest [`LedOutput::observe`]; read by the engine
    /// about once a second.
    issue: Mutex<Option<OutputIssue>>,
}

impl Control {
    fn report(&self, issue: Option<OutputIssue>) {
        *self.issue.lock().unwrap_or_else(PoisonError::into_inner) = issue;
    }
}

/// The session thread's state between frames.
///
/// Each frame: count, apply a pending config, process, open the output if not
/// held, write, observe, preview (only while watched). A quiet tick rewrites
/// (and observes) the last colors, and after [`VIDEO_GRACE`] without frames
/// idles the output in [`RELEASE`] and closes it until the next frame. When
/// the loop ends (stop, failure, or [`Pull::Ended`](prysm_ports::Pull::Ended))
/// the preview is cleared, `source.close()`, and the output is left in the
/// stop's idle mode (idled if held, reopened only if it was left in another
/// mode; never opened for `Off` if no frame came).
struct SessionLoop {
    processor: PrysmProcessor,
    leds: Leds,
    /// The last colors written and when.
    last: Option<(EdgeColors, Instant)>,
    last_frame: Instant,
    open_output: OpenOutput,
    control: Arc<Control>,
    preview: Arc<PreviewSlot>,
    stats: Arc<Stats>,
}

impl SessionLoop {
    fn on_frame(&mut self, frame: Frame) -> Result<()> {
        self.last_frame = Instant::now();
        self.stats.frame_received();
        if let Some(config) = self.control.config.take() {
            self.processor.configure(&config);
        }
        let shown = (self.preview.viewers() > 0).then(|| frame.clone());
        let colors = self.stats.process(|| self.processor.process_frame(frame));
        self.leds
            .write(&colors, &mut self.open_output, &self.stats)?;
        self.stats.frame_sent();
        self.control.report(self.leds.observe());
        self.last = Some((colors.clone(), Instant::now()));
        if let Some(shown) = shown {
            let viewport = self
                .processor
                .viewport()
                .unwrap_or_else(|| Viewport::full_frame(shown.width(), shown.height()));
            self.preview.publish(shown, colors, viewport);
        }
        Ok(())
    }

    fn on_quiet(&mut self) -> Result<()> {
        let Some(output) = self.leds.held() else {
            return Ok(());
        };
        if self.last_frame.elapsed() >= VIDEO_GRACE {
            self.last = None;
            self.control.report(None);
            return self.leds.release(RELEASE);
        }
        if let Some((colors, written)) = &mut self.last
            && written.elapsed() >= REFRESH
        {
            self.stats.write(output, colors)?;
            *written = Instant::now();
            self.control.report(output.observe());
        }
        Ok(())
    }

    fn finish(mut self, pumped: Pumped, source: Box<dyn FrameSource>, flags: &Flags) -> SessionEnd {
        let ended = matches!(pumped, Pumped::Ended);
        let failure = pumped.failure();
        self.control.report(None);
        self.preview.clear();
        flags.set_phase(SessionPhase::ClosingSource);
        let released = source.close();
        flags.set_phase(SessionPhase::IdlingOutput);
        let mode = *self
            .control
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let idled = self.leds.finish(mode, &mut self.open_output);
        SessionEnd {
            failure,
            ended,
            released,
            idled,
        }
    }
}

/// One dedicated thread owning a source, the processor and an output; the
/// rules are [`SessionLoop`]'s.
pub(crate) struct Session {
    control: Arc<Control>,
    worker: Worker<SessionEnd>,
}

impl Session {
    /// # Errors
    /// The thread could not be spawned. The source comes back unclosed, so the
    /// caller can still release it.
    pub(crate) fn start(
        source: Box<dyn FrameSource>,
        open_output: OpenOutput,
        config: Config,
        preview: Arc<PreviewSlot>,
        stats: Arc<Stats>,
    ) -> Result<Self, (Error, Box<dyn FrameSource>)> {
        let control = Arc::new(Control {
            config: LatestSlot::new(),
            idle: Mutex::new(IdleMode::Off),
            issue: Mutex::new(None),
        });
        let session = SessionLoop {
            processor: PrysmProcessor::new(&config),
            leds: Leds::Untouched,
            last: None,
            last_frame: Instant::now(),
            open_output,
            control: control.clone(),
            preview,
            stats,
        };
        let worker = Worker::spawn(
            "prysm-session",
            (source, session),
            |(mut source, mut session), flags| {
                let pumped = flags.pump(&mut *source, |frame| match frame {
                    Some(frame) => session.on_frame(frame),
                    None => session.on_quiet(),
                });
                session.finish(pumped, source, flags)
            },
        )
        .map_err(|(error, (source, _))| (error, source))?;
        Ok(Self { control, worker })
    }

    /// Apply processing settings at the next frame boundary.
    pub fn configure(&self, config: Config) {
        self.control.config.offer(config);
    }

    /// Why the held output's LEDs are not showing the session's colours.
    #[must_use]
    pub fn output_issue(&self) -> Option<OutputIssue> {
        *self
            .control
            .issue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait for the session to end on its own (a failure or the source
    /// ending). Cancel-safe.
    ///
    /// # Errors
    /// The thread panicked while releasing its adapters.
    pub async fn finished(&mut self) -> Result<SessionEnd> {
        self.worker.finished().await
    }

    /// Stop, release, idle with `mode` and join within `deadline`.
    ///
    /// # Errors
    /// [`Error::Faulted`] with the phase the thread is stuck in when the
    /// deadline passes (the thread is then abandoned), or [`Error::Panicked`].
    pub async fn stop(self, mode: IdleMode, deadline: Duration) -> Result<SessionEnd> {
        *self
            .control
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = mode;
        self.worker.stop(deadline).await.map_err(Error::Faulted)?
    }

    /// Join after [`Session::finished`] returned.
    pub async fn join(self) {
        self.worker.join().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::PixelFormat;
    use prysm_ports::Pull;
    use std::sync::atomic::AtomicBool;

    /// A uniform grey frame every millisecond; counts frames it handed out.
    struct Grey(Arc<AtomicUsize>);

    impl FrameSource for Grey {
        fn next_frame(&mut self, _timeout: Duration) -> Result<Pull> {
            std::thread::sleep(Duration::from_millis(1));
            self.0.fetch_add(1, Ordering::SeqCst);
            Frame::fill(128, 32, 18, PixelFormat::RGB24)
                .map(Pull::Frame)
                .map_err(|error| Error::adapter("grey frame", error))
        }
        fn close(self: Box<Self>) -> Result<()> {
            Ok(())
        }
    }

    struct Sink(Arc<AtomicBool>);

    impl LedOutput for Sink {
        fn write(&mut self, _colors: &EdgeColors) -> Result<()> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn idle(self: Box<Self>, _mode: IdleMode) -> Result<()> {
            Ok(())
        }
    }

    fn start(slot: &Arc<PreviewSlot>) -> (Session, Arc<AtomicUsize>) {
        let frames = Arc::new(AtomicUsize::new(0));
        let session = Session::start(
            Box::new(Grey(frames.clone())),
            Box::new(|| Ok(Box::new(Sink(Arc::new(AtomicBool::new(false)))) as Box<dyn LedOutput>)),
            Config::default(),
            slot.clone(),
            Arc::new(Stats::default()),
        )
        .map_err(|(error, _)| error)
        .unwrap();
        (session, frames)
    }

    async fn frames_after(frames: &AtomicUsize, count: usize) {
        let start = frames.load(Ordering::SeqCst);
        while frames.load(Ordering::SeqCst) < start + count {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    async fn latest(watch: &PreviewWatch, after: u64) -> Preview {
        loop {
            if let Some(preview) = watch.latest().filter(|p| p.sequence > after) {
                return preview;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    #[tokio::test]
    async fn unwatched_sessions_publish_nothing() {
        let slot = Arc::new(PreviewSlot::default());
        let (session, frames) = start(&slot);
        frames_after(&frames, 20).await;
        assert!(slot.latest.peek().is_none());
        assert_eq!(slot.sequence.load(Ordering::SeqCst), 1, "nothing published");

        let watch = slot.watch();
        let first = latest(&watch, 0).await;
        drop(watch);
        assert_eq!(slot.viewers(), 0);
        // A frame already past the check may still be published, then dropped.
        frames_after(&frames, 5).await;
        assert!(
            slot.latest.peek().is_none(),
            "the last viewer releases the frame"
        );
        let published = slot.sequence.load(Ordering::SeqCst);
        frames_after(&frames, 20).await;
        assert_eq!(slot.sequence.load(Ordering::SeqCst), published);
        assert!(first.sequence >= 1);
        session
            .stop(IdleMode::Off, Duration::from_secs(2))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn watched_previews_carry_viewport_and_increasing_sequence() {
        let slot = Arc::new(PreviewSlot::default());
        let watch = slot.watch();
        let (session, _) = start(&slot);
        let first = latest(&watch, 0).await;
        assert_eq!((first.frame.width(), first.frame.height()), (32, 18));
        assert_eq!(first.viewport, Viewport::full_frame(32, 18));
        let second = latest(&watch, first.sequence).await;
        assert!(second.sequence > first.sequence);
        session
            .stop(IdleMode::Off, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(
            watch.latest().is_none(),
            "no preview once the session ended"
        );

        // A later session continues the sequence, so viewers never skip it.
        let (session, _) = start(&slot);
        let next = latest(&watch, 0).await;
        assert!(next.sequence > second.sequence);
        session
            .stop(IdleMode::Off, Duration::from_secs(2))
            .await
            .unwrap();
    }
}
