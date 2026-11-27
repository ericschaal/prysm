//! What a session shows its viewers: the latest preview and the colors last
//! written, each published only while someone watches.
use prysm_core::{EdgeColors, Frame, Viewport};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::watch;

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
    latest: watch::Sender<Option<T>>,
    watchers: AtomicUsize,
    sequence: AtomicU64,
}

impl<T> Default for Feed<T> {
    fn default() -> Self {
        Self {
            latest: watch::Sender::new(None),
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
        Watch {
            feed: self.clone(),
            receiver: self.latest.subscribe(),
        }
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
        let value = make(sequence);
        // A frame can finish after the last viewer leaves. Check demand while
        // holding the channel's write lock, so its clear cannot race our write.
        self.latest.send_modify(|latest| {
            *latest = self.watched().then_some(value);
        });
    }

    fn clear(&self) {
        self.latest.send_replace(None);
    }
}

/// A registered watcher of a feed. Cheap to hold; dropping the last one stops
/// publication and releases the held value.
#[derive(Debug)]
pub struct Watch<T> {
    feed: Arc<Feed<T>>,
    receiver: watch::Receiver<Option<T>>,
}

impl<T: Clone> Watch<T> {
    /// The latest value without taking it: other watchers see it too.
    /// `None` while no session runs, before its first frame, or (colors)
    /// while its output is released.
    #[must_use]
    pub fn latest(&self) -> Option<T> {
        self.receiver.borrow().clone()
    }
}

impl<T> Watch<T> {
    /// Wait until a value was published or cleared since this watch was
    /// made or last waited; [`latest`](Self::latest) then holds it. Several
    /// changes meanwhile end one wait. Cancel-safe.
    pub async fn changed(&mut self) {
        // This watch holds the feed, which holds the sender: it never closes.
        let _ = self.receiver.changed().await;
    }
}

impl<T> Drop for Watch<T> {
    fn drop(&mut self) {
        // Decide whether this is the last viewer under the same lock as the
        // clear. A replacement viewer's first frame must survive this drop.
        self.feed.latest.send_if_modified(|latest| {
            if self.feed.watchers.fetch_sub(1, Ordering::SeqCst) == 1 {
                *latest = None;
                return true;
            }
            false
        });
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
    use crate::null::{NullOutput, NullSettings};
    use crate::session::{Notice, Session, Start};
    use crate::session_lifecycle::OutputRelease;
    use crate::stats::Stats;
    use prysm_core::{Config, PixelFormat};
    use prysm_ports::{
        AdapterSettings, Error, FrameSource, OpenIntent, OutputFactory, Pull, Registry, Result,
    };
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

    fn ready_claim() -> prysm_ports::Claim {
        let mut adapter = prysm_ports::AdapterLifecycle::<()>::default();
        adapter.start(prysm_ports::Readiness::Ready);
        adapter.claim().unwrap()
    }

    async fn open_capture(feeds: &Feeds) -> (Session, Arc<AtomicUsize>) {
        let frames = Arc::new(AtomicUsize::new(0));
        let mut registry = Registry::default();
        registry.add_output(NullOutput);
        let source = frames.clone();
        let mut session = Session::spawn(
            ready_claim(),
            Start {
                output: registry
                    .output(&AdapterSettings::new(
                        NullOutput::KIND,
                        &NullSettings::new(),
                    ))
                    .unwrap(),
                config: Config::default(),
                feeds: feeds.clone(),
                stats: Arc::new(Stats::default()),
            },
            move |_, _| Ok(Box::new(Grey(source))),
        )
        .unwrap();
        assert!(matches!(session.next().await, Notice::CaptureOpened));
        (session, frames)
    }

    async fn start(feeds: &Feeds) -> (Session, Arc<AtomicUsize>) {
        let (mut session, frames) = open_capture(feeds).await;
        session.open_output(OpenIntent::Resume, ready_claim());
        assert!(matches!(session.next().await, Notice::OutputOpened));
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

    /// A watch wakes for each publish and clear, and only then.
    #[tokio::test]
    async fn a_watch_is_told_of_every_publish_and_clear() {
        use futures::FutureExt;
        let feed: Arc<Feed<u64>> = Arc::default();
        let mut watch = feed.watch();
        assert!(watch.changed().now_or_never().is_none(), "nothing yet");
        feed.publish(|sequence| sequence);
        feed.publish(|sequence| sequence);
        assert!(watch.changed().now_or_never().is_some());
        assert_eq!(watch.latest(), Some(2));
        assert!(watch.changed().now_or_never().is_none(), "both seen");
        feed.clear();
        assert!(watch.changed().now_or_never().is_some());
        assert_eq!(watch.latest(), None);
    }

    #[test]
    fn a_value_finished_after_the_last_viewer_left_is_released() {
        let feed: Arc<Feed<u64>> = Arc::default();
        let viewer = feed.watch();
        feed.publish(|sequence| {
            drop(viewer);
            sequence
        });
        assert_eq!(feed.watch().latest(), None);
    }

    #[test]
    fn replacing_a_viewer_preserves_the_new_viewers_first_frame() {
        let feed: Arc<Feed<u64>> = Arc::default();
        let previous = feed.watch();
        let ready = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let departing = scope.spawn(|| {
                ready.wait();
                drop(previous);
            });
            ready.wait();
            let next = feed.watch();
            feed.publish(|_| 42);
            departing.join().unwrap();
            assert_eq!(next.latest(), Some(42));
        });
    }

    async fn stop(mut session: Session) {
        session.request_stop();
        assert!(matches!(session.next().await, Notice::Ended(_)));
        assert!(matches!(
            session.next().await,
            Notice::IdleReady {
                capture_released: true
            }
        ));
        session.idle(OutputRelease::Preserve, None);
        let Notice::Finished(Ok(end)) = session.next().await else {
            panic!("session did not finish");
        };
        end.capture.unwrap();
        end.output.unwrap();
        session.join().await;
    }

    #[tokio::test]
    async fn stop_before_output_authorization_releases_capture_without_streaming() {
        let (session, frames) = open_capture(&Feeds::default()).await;
        tokio::time::timeout(Duration::from_secs(5), stop(session))
            .await
            .expect("stopping must wake the worker waiting for output authorization");
        assert_eq!(frames.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unwatched_sessions_publish_nothing() {
        let feeds = Feeds::default();
        let (session, frames) = start(&feeds).await;
        frames_after(&frames, 20).await;
        assert!(feeds.preview.latest.borrow().is_none());
        assert!(feeds.colors.latest.borrow().is_none());
        assert_eq!(published(&feeds.preview), 0, "nothing published");
        assert_eq!(published(&feeds.colors), 0, "nothing published");

        let watch = feeds.preview.watch();
        let first = latest(&watch, 0).await;
        drop(watch);
        assert_eq!(feeds.preview.watchers(), 0);
        frames_after(&frames, 5).await;
        assert!(
            feeds.preview.latest.borrow().is_none(),
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
        let (session, _) = start(&feeds).await;
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

        let (session, _) = start(&feeds).await;
        let next = latest(&watch, 0).await;
        assert!(next.sequence > second.sequence);
        stop(session).await;
    }

    #[tokio::test]
    async fn colors_and_preview_are_published_independently() {
        let feeds = Feeds::default();
        let colors = feeds.colors.watch();
        let (session, frames) = start(&feeds).await;
        frames_after(&frames, 20).await;
        let written = colors.latest().expect("colors are published");
        assert!(!written.top.is_empty());
        assert_eq!(published(&feeds.preview), 0);
        assert!(feeds.preview.latest.borrow().is_none());

        drop(colors);
        frames_after(&frames, 5).await;
        assert!(
            feeds.colors.latest.borrow().is_none(),
            "the last watch clears"
        );
        let count = published(&feeds.colors);
        let preview = feeds.preview.watch();
        latest(&preview, 0).await;
        frames_after(&frames, 20).await;
        assert_eq!(published(&feeds.colors), count);
        assert!(feeds.colors.latest.borrow().is_none());

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
