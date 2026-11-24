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

    /// Availability and context generation (the number of resets).
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
        feed.wakeups.retain(Wakeup::wake);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Feed> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
