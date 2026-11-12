use crate::worker::{Pumped, Worker};
use prysm_core::{Config, EdgeColors, Frame, Viewport};
use prysm_ports::{Error, FrameSource, IdleMode, LatestSlot, LedOutput, Result, SessionPhase};
use prysm_processor::PrysmProcessor;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A quiet source must not let the device time out of live mode: the last
/// colors are rewritten at this interval.
const REFRESH: Duration = Duration::from_millis(500);

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
pub struct PreviewSlot {
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
    pub fn watch(self: &Arc<Self>) -> PreviewWatch {
        self.viewers.fetch_add(1, Ordering::SeqCst);
        PreviewWatch(self.clone())
    }

    /// How many watches exist.
    #[must_use]
    pub fn viewers(&self) -> usize {
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
pub struct SessionEnd {
    /// Why the loop stopped; `None` when it was asked to or the source ended.
    pub failure: Option<Error>,
    /// The source reported [`Pull::Ended`](prysm_ports::Pull::Ended): the
    /// session finished on its own without a failure.
    pub ended: bool,
    pub released: Result<()>,
    pub idled: Result<()>,
}

impl SessionEnd {
    /// The first failure, in the order it happened.
    ///
    /// # Errors
    /// The loop failure, else the release failure, else the idle failure.
    pub fn result(self) -> Result<()> {
        match self.failure {
            Some(failure) => Err(failure),
            None => self.released.and(self.idled),
        }
    }
}

struct Control {
    config: LatestSlot<Config>,
    idle: Mutex<IdleMode>,
    /// Frames taken from the source; read by the engine about once a second.
    frames: AtomicU64,
}

/// One dedicated thread owning a source, the processor and an output.
///
/// Loop: stop flag, pending config at the frame boundary, `next_frame`,
/// count, process, write, preview (only while watched); a quiet tick rewrites
/// the last colors; [`Pull::Ended`](prysm_ports::Pull::Ended) leaves the
/// loop like a stop. Then the preview is cleared, `source.close()` and
/// `output.idle(mode)`.
pub struct Session {
    control: Arc<Control>,
    worker: Worker<SessionEnd>,
}

impl Session {
    /// # Errors
    /// The thread could not be spawned; the adapters are dropped unreleased.
    pub fn start(
        mut source: Box<dyn FrameSource>,
        mut output: Box<dyn LedOutput>,
        config: Config,
        preview: Arc<PreviewSlot>,
    ) -> Result<Self> {
        let control = Arc::new(Control {
            config: LatestSlot::new(),
            idle: Mutex::new(IdleMode::Off),
            frames: AtomicU64::new(0),
        });
        let shared = control.clone();
        let worker = Worker::spawn("prysm-session", move |flags| {
            let mut processor = PrysmProcessor::new(&config);
            let mut last: Option<(EdgeColors, Instant)> = None;
            let pumped = flags.pump(&mut *source, |frame| {
                let Some(frame) = frame else {
                    if let Some((colors, written)) = &mut last
                        && written.elapsed() >= REFRESH
                    {
                        output.write(colors)?;
                        *written = Instant::now();
                    }
                    return Ok(());
                };
                shared.frames.fetch_add(1, Ordering::Relaxed);
                if let Some(config) = shared.config.take() {
                    processor.configure(&config);
                }
                let shown = (preview.viewers() > 0).then(|| frame.clone());
                let colors = processor.process_frame(frame);
                output.write(&colors)?;
                last = Some((colors.clone(), Instant::now()));
                if let Some(shown) = shown {
                    let viewport = processor
                        .viewport()
                        .unwrap_or_else(|| Viewport::full_frame(shown.width(), shown.height()));
                    preview.publish(shown, colors, viewport);
                }
                Ok(())
            });
            let ended = matches!(pumped, Pumped::Ended);
            let failure = pumped.failure();
            preview.clear();
            flags.set_phase(SessionPhase::ClosingSource);
            let released = source.close();
            flags.set_phase(SessionPhase::IdlingOutput);
            let mode = *shared.idle.lock().unwrap_or_else(PoisonError::into_inner);
            let idled = output.idle(mode);
            SessionEnd {
                failure,
                ended,
                released,
                idled,
            }
        })?;
        Ok(Self { control, worker })
    }

    /// Apply processing settings at the next frame boundary.
    pub fn configure(&self, config: Config) {
        self.control.config.offer(config);
    }

    /// Frames taken from the source since the session started.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.control.frames.load(Ordering::Relaxed)
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
            Box::new(Sink(Arc::new(AtomicBool::new(false)))),
            Config::default(),
            slot.clone(),
        )
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
