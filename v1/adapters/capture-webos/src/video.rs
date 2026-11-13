pub use prysm_webos_link::Video;
use std::sync::{Arc, Mutex, PoisonError};

/// Shared readiness feed from the host's Luna observer to every open source.
/// Cloning shares the same state.
#[derive(Debug, Clone, Default)]
pub struct VideoSignal(Arc<Mutex<Readiness>>);

#[derive(Debug, Clone, Copy, Default)]
struct Readiness {
    video: Video,
    /// Bumped by every [`VideoSignal::reset`].
    generation: u64,
}

impl VideoSignal {
    pub fn set(&self, video: Video) {
        self.lock(|state| state.video = video);
    }

    /// The video context changed (input, app or mode): release and reopen
    /// the native tap.
    pub fn reset(&self) {
        self.lock(|state| state.generation = state.generation.wrapping_add(1));
    }

    /// Availability and context generation (the number of resets).
    #[must_use]
    pub fn current(&self) -> (Video, u64) {
        self.lock(|state| (state.video, state.generation))
    }

    fn lock<T>(&self, f: impl FnOnce(&mut Readiness) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
