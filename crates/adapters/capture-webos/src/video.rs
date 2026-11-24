use prysm_ports::Wakeup;
pub use prysm_webos_link::Video;
use std::sync::{Arc, Mutex, PoisonError};

/// Shared readiness feed from the host's Luna observer to every open source.
/// Cloning shares the same state. Each change wakes the threads of the open
/// sources that follow it.
#[derive(Debug, Clone, Default)]
pub struct VideoSignal(Arc<Mutex<Feed>>);

#[derive(Debug, Default)]
struct Feed {
    readiness: Readiness,
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
    /// Bumped by every change of the signal (each [`VideoSignal::set`],
    /// [`VideoSignal::reset`] and [`VideoSignal::permission_changed`]), so a
    /// reader that compares it sees two changes in a row that leave `video`
    /// and `generation` as they were.
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

    /// The TV's capture permission changed: a driver whose initialization
    /// was refused is initialized once more. A running tap is left alone.
    pub fn permission_changed(&self) {
        self.change(|_| {});
    }

    /// Availability, context generation (the number of resets) and change
    /// sequence.
    #[must_use]
    pub fn current(&self) -> Readiness {
        self.lock().readiness
    }

    /// Wake `wakeup` at every change from now on.
    pub(crate) fn wake(&self, wakeup: Wakeup) {
        self.lock().wakeups.push(wakeup);
    }

    fn change(&self, change: impl FnOnce(&mut Readiness)) {
        let mut feed = self.lock();
        change(&mut feed.readiness);
        feed.readiness.sequence = feed.readiness.sequence.wrapping_add(1);
        feed.wakeups.retain(Wakeup::wake);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Feed> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
