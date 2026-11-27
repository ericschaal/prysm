use crate::WebosSettings;
use prysm_ports::{Error, Hangup, Readiness as Availability, Wakeup};
pub use prysm_webos_link::Video;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use tokio::sync::watch;

/// Shared readiness feed from the host's Luna observer to every open source.
/// Cloning shares the same state. Each change wakes the threads of the open
/// sources that follow it.
#[derive(Debug, Clone, Default)]
pub struct VideoSignal(Arc<Signals>);

#[derive(Debug, Default)]
struct Signals {
    feed: Mutex<Feed>,
    changed: Condvar,
    initialized: watch::Sender<bool>,
}

#[derive(Debug, thiserror::Error)]
#[error("the replacement video driver also stalled")]
pub struct RepeatedStall(#[source] Error);

#[derive(Debug, Default)]
struct Feed {
    readiness: Readiness,
    observed: bool,
    stall: Option<(WebosSettings, Error)>,
    /// The threads that follow the signal; one that is gone is forgotten at
    /// the next change.
    wakeups: Vec<Wakeup>,
}

/// What a [`VideoSignal`] says now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Readiness {
    pub video: Video,
    /// The video context: bumped by every [`VideoSignal::reset`].
    pub generation: u64,
    /// Bumped by every change of the signal (each [`VideoSignal::set`] and
    /// [`VideoSignal::reset`]), so a reader that compares it sees two changes
    /// in a row that leave `video` and `generation` as they were.
    pub sequence: u64,
}

impl VideoSignal {
    pub fn set(&self, video: Video) {
        self.change(|readiness| readiness.video = video);
    }

    /// The video context changed (input, app or mode): release and reopen
    /// the native tap, or initialize again a tap whose video the TV refused.
    pub fn reset(&self) {
        self.change(|readiness| readiness.generation = readiness.generation.wrapping_add(1));
    }

    /// Availability, context generation (the number of resets) and change
    /// sequence.
    #[must_use]
    pub fn current(&self) -> Readiness {
        self.lock().readiness
    }

    /// Whether the supervisor has a live video subscription.
    pub fn observe(&self, available: bool) {
        self.lock().observed = available;
        self.0.changed.notify_all();
    }

    /// Whether waiting capture can observe future video changes.
    #[must_use]
    fn can_observe(&self) -> bool {
        self.lock().observed
    }

    /// Wait until the worker has initialized a native capture tap.
    pub async fn initialized(&self) {
        let _ = self
            .0
            .initialized
            .subscribe()
            .wait_for(|ready| *ready)
            .await;
    }

    pub(crate) fn mark_initialized(&self) {
        self.0.initialized.send_if_modified(|ready| {
            let changed = !*ready;
            *ready = true;
            changed
        });
    }

    pub(crate) fn stalled(&self, sequence: u64, settings: WebosSettings, cause: Error) -> Error {
        let error = Error::adapter("webOS video capture stalled", RepeatedStall(cause));
        let mut feed = self.lock();
        if feed.readiness.sequence == sequence {
            feed.stall = Some((settings, error.clone()));
        }
        error
    }

    /// The capture's current availability after a repeated video stall.
    #[must_use]
    pub fn capture_readiness(&self, settings: &WebosSettings) -> Availability<Error> {
        self.lock().capture_readiness(settings)
    }

    pub(crate) fn failure(&self, error: Error) -> Availability<Error> {
        if error.cause::<RepeatedStall>().is_some() && self.can_observe() {
            Availability::Waiting(error)
        } else {
            Availability::Blocked(error)
        }
    }

    pub(crate) fn watch_stall(
        &self,
        settings: &WebosSettings,
        stop: &Hangup,
    ) -> Option<Availability<Error>> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let _registered = stop.on_cut({
            let cancelled = Arc::clone(&cancelled);
            let signal = self.clone();
            move || {
                let _locked = signal.lock();
                cancelled.store(true, Ordering::Release);
                signal.0.changed.notify_all();
            }
        })?;
        let feed = self
            .0
            .changed
            .wait_while(self.lock(), |feed| {
                !cancelled.load(Ordering::Acquire)
                    && feed.observed
                    && feed.stall_for(settings).is_some()
            })
            .unwrap_or_else(PoisonError::into_inner);
        (!cancelled.load(Ordering::Acquire)).then(|| feed.capture_readiness(settings))
    }

    /// Wake `wakeup` at every change from now on.
    pub(crate) fn wake(&self, wakeup: Wakeup) {
        self.lock().wakeups.push(wakeup);
    }

    fn change(&self, change: impl FnOnce(&mut Readiness)) {
        let mut feed = self.lock();
        change(&mut feed.readiness);
        feed.readiness.sequence = feed.readiness.sequence.wrapping_add(1);
        feed.stall = None;
        feed.wakeups.retain(Wakeup::wake);
        self.0.changed.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Feed> {
        self.0.feed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Feed {
    fn stall_for(&self, settings: &WebosSettings) -> Option<&Error> {
        self.stall
            .as_ref()
            .filter(|(stalled, _)| stalled == settings)
            .map(|(_, error)| error)
    }

    fn capture_readiness(&self, settings: &WebosSettings) -> Availability<Error> {
        match self.stall_for(settings) {
            Some(error) if self.observed => Availability::Waiting(error.clone()),
            Some(error) => Availability::Blocked(error.clone()),
            None => Availability::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn stall_monitor_finishes_only_on_signal_loss_change_or_cancellation() {
        for action in ["reset", "disconnect", "cancel"] {
            let video = VideoSignal::default();
            let settings = WebosSettings::default();
            video.observe(true);
            let pending = video.stalled(0, settings, Error::other("no frame"));
            assert!(matches!(video.failure(pending), Availability::Waiting(_)));
            let different = WebosSettings {
                width: 320,
                ..settings
            };
            assert!(matches!(
                video.capture_readiness(&different),
                Availability::Ready
            ));

            let stop = Hangup::default();
            let (send, result) = mpsc::channel();
            let task = {
                let video = video.clone();
                let stop = stop.clone();
                thread::spawn(move || send.send(video.watch_stall(&settings, &stop)).unwrap())
            };
            assert!(result.try_recv().is_err());
            match action {
                "reset" => video.reset(),
                "disconnect" => video.observe(false),
                "cancel" => stop.cut(),
                _ => unreachable!(),
            }
            let received = result.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(matches!(
                (action, received),
                ("reset", Some(Availability::Ready))
                    | ("disconnect", Some(Availability::Blocked(_)))
                    | ("cancel", None)
            ));
            task.join().unwrap();
        }
    }
}
