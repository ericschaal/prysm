//! What a session shows its viewers: the latest preview and the colors last
//! written, each published only while someone watches.
use prysm_core::{EdgeColors, Frame, Viewport};
use prysm_ports::LatestSlot;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

/// Values the session publishes only while at least one [`Watch`] exists, so
/// nobody watching costs one atomic load per frame and holds nothing. Empty
/// while no session runs.
#[derive(Debug)]
pub(crate) struct Feed<T> {
    latest: LatestSlot<T>,
    watchers: AtomicUsize,
    sequence: AtomicU64,
}

impl<T> Default for Feed<T> {
    fn default() -> Self {
        Self {
            latest: LatestSlot::new(),
            watchers: AtomicUsize::new(0),
            sequence: AtomicU64::new(1),
        }
    }
}

impl<T> Feed<T> {
    /// Register a watcher until the returned watch is dropped.
    #[must_use]
    pub(crate) fn watch(self: &Arc<Self>) -> Watch<T> {
        self.watchers.fetch_add(1, Ordering::SeqCst);
        Watch(self.clone())
    }

    /// How many watches exist.
    #[must_use]
    pub(crate) fn watchers(&self) -> usize {
        self.watchers.load(Ordering::SeqCst)
    }

    fn watched(&self) -> bool {
        self.watchers() > 0
    }

    /// Offer the value `make` builds from the next sequence number, which
    /// increases with every published value, across sessions.
    fn publish(&self, make: impl FnOnce(u64) -> T) {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.latest.offer(make(sequence));
        // The last watch may have gone between the check and the offer; its
        // clear could have run first, so do not leave the value pinned.
        if !self.watched() {
            self.latest.take();
        }
    }

    fn clear(&self) {
        self.latest.take();
    }
}

/// A registered watcher of a feed. Cheap to hold; dropping the last one stops
/// publication and releases the held value.
#[derive(Debug)]
pub struct Watch<T>(Arc<Feed<T>>);

impl<T: Clone> Watch<T> {
    /// The latest value without taking it: other watchers see it too.
    /// `None` while no session runs, before its first frame, or (colors)
    /// while its output is released.
    #[must_use]
    pub fn latest(&self) -> Option<T> {
        self.0.latest.peek()
    }
}

impl<T> Drop for Watch<T> {
    fn drop(&mut self) {
        if self.0.watchers.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.clear();
        }
    }
}

/// Watches the latest processed frame with its colors; publishing one clones
/// the frame.
pub type PreviewWatch = Watch<Preview>;

/// Watches the colors last written to the LEDs, without the frame: publishing
/// clones only the colors. Empty whenever the LEDs are not showing the
/// session's colors (no session, or its output released for lack of frames).
pub type ColorsWatch = Watch<EdgeColors>;

/// The feeds a session publishes to; they outlive sessions.
#[derive(Debug, Clone, Default)]
pub(crate) struct Feeds {
    pub(crate) preview: Arc<Feed<Preview>>,
    pub(crate) colors: Arc<Feed<EdgeColors>>,
}

impl Feeds {
    /// A copy of `frame` for the preview, only while someone watches it
    /// (taken before processing consumes the frame).
    pub(crate) fn preview_frame(&self, frame: &Frame) -> Option<Frame> {
        self.preview.watched().then(|| frame.clone())
    }

    /// Publish what is watched: the colors, and the preview of `shown` (from
    /// [`Self::preview_frame`]) sampled at `viewport` (the whole frame when
    /// the processor rejected it). Nothing is copied for a feed nobody
    /// watches.
    pub(crate) fn publish(
        &self,
        colors: &EdgeColors,
        shown: Option<Frame>,
        viewport: Option<Viewport>,
    ) {
        if self.colors.watched() {
            self.colors.publish(|_| colors.clone());
        }
        if let Some(frame) = shown {
            let viewport =
                viewport.unwrap_or_else(|| Viewport::full_frame(frame.width(), frame.height()));
            self.preview.publish(|sequence| Preview {
                frame,
                colors: colors.clone(),
                viewport,
                sequence,
            });
        }
    }

    /// The device no longer shows the session's colors.
    pub(crate) fn clear_colors(&self) {
        self.colors.clear();
    }

    pub(crate) fn clear(&self) {
        self.preview.clear();
        self.colors.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;
    use crate::stats::Stats;
    use prysm_core::{Config, PixelFormat};
    use prysm_ports::{Error, FrameSource, IdleMode, LedOutput, Pull, Result};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

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

    fn start(feeds: &Feeds) -> (Session, Arc<AtomicUsize>) {
        let frames = Arc::new(AtomicUsize::new(0));
        let session = Session::start(
            Box::new(Grey(frames.clone())),
            Box::new(|| Ok(Box::new(Sink(Arc::new(AtomicBool::new(false)))) as Box<dyn LedOutput>)),
            Config::default(),
            feeds.clone(),
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

    fn published<T>(feed: &Feed<T>) -> u64 {
        feed.sequence.load(Ordering::SeqCst) - 1
    }

    async fn stop(session: Session) {
        session
            .stop(IdleMode::Off, Duration::from_secs(2))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unwatched_sessions_publish_nothing() {
        let feeds = Feeds::default();
        let (session, frames) = start(&feeds);
        frames_after(&frames, 20).await;
        assert!(feeds.preview.latest.peek().is_none());
        assert!(feeds.colors.latest.peek().is_none());
        assert_eq!(published(&feeds.preview), 0, "nothing published");
        assert_eq!(published(&feeds.colors), 0, "nothing published");

        let watch = feeds.preview.watch();
        let first = latest(&watch, 0).await;
        drop(watch);
        assert_eq!(feeds.preview.watchers(), 0);
        // A frame already past the check may still be published, then dropped.
        frames_after(&frames, 5).await;
        assert!(
            feeds.preview.latest.peek().is_none(),
            "the last viewer releases the frame"
        );
        let count = published(&feeds.preview);
        frames_after(&frames, 20).await;
        assert_eq!(published(&feeds.preview), count);
        assert!(first.sequence >= 1);
        stop(session).await;
    }

    #[tokio::test]
    async fn watched_previews_carry_viewport_and_increasing_sequence() {
        let feeds = Feeds::default();
        let watch = feeds.preview.watch();
        let (session, _) = start(&feeds);
        let first = latest(&watch, 0).await;
        assert_eq!((first.frame.width(), first.frame.height()), (32, 18));
        assert_eq!(first.viewport, Viewport::full_frame(32, 18));
        let second = latest(&watch, first.sequence).await;
        assert!(second.sequence > first.sequence);
        stop(session).await;
        assert!(
            watch.latest().is_none(),
            "no preview once the session ended"
        );

        // A later session continues the sequence, so viewers never skip it.
        let (session, _) = start(&feeds);
        let next = latest(&watch, 0).await;
        assert!(next.sequence > second.sequence);
        stop(session).await;
    }

    #[tokio::test]
    async fn colors_and_preview_are_published_independently() {
        let feeds = Feeds::default();
        let colors = feeds.colors.watch();
        let (session, frames) = start(&feeds);
        frames_after(&frames, 20).await;
        let written = colors.latest().expect("colors are published");
        assert!(!written.top.is_empty());
        // Watching colors copies no frame and publishes no preview.
        assert_eq!(published(&feeds.preview), 0);
        assert!(feeds.preview.latest.peek().is_none());

        drop(colors);
        // A frame already past the check may still be published, then dropped.
        frames_after(&frames, 5).await;
        assert!(
            feeds.colors.latest.peek().is_none(),
            "the last watch clears"
        );
        let count = published(&feeds.colors);
        // Watching the preview publishes no colors.
        let preview = feeds.preview.watch();
        latest(&preview, 0).await;
        frames_after(&frames, 20).await;
        assert_eq!(published(&feeds.colors), count);
        assert!(feeds.colors.latest.peek().is_none());

        let colors = feeds.colors.watch();
        frames_after(&frames, 5).await;
        assert!(colors.latest().is_some());
        stop(session).await;
        assert!(
            colors.latest().is_none(),
            "no colors once the session ended"
        );
        assert!(preview.latest().is_none());
    }
}
