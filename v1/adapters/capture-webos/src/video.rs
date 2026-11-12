use std::sync::{Arc, Mutex, PoisonError};

/// The TV's main video output, as Luna `videooutput/getStatus` reports the
/// `MAIN` sink.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Video {
    /// No report yet. Capture is attempted, bounded by the ten-second
    /// preparation limit.
    #[default]
    Unknown,
    /// Disconnected or zero-sized. The native tap is released and frames stop.
    Unavailable,
    /// Connected with a size and frame rate. Muted video still counts.
    Available,
}

/// Shared readiness feed from the host's Luna observer to every open source.
/// Cloning shares the same state.
#[derive(Debug, Clone, Default)]
pub struct VideoSignal(Arc<Mutex<(Video, u64)>>);

impl VideoSignal {
    pub fn set(&self, video: Video) {
        self.lock(|state| state.0 = video);
    }

    /// The video context changed (input, app or mode): release and reopen
    /// the native tap.
    pub fn reset(&self) {
        self.lock(|state| state.1 = state.1.wrapping_add(1));
    }

    /// Availability and context generation.
    pub(crate) fn current(&self) -> (Video, u64) {
        self.lock(|state| *state)
    }

    fn lock<T>(&self, f: impl FnOnce(&mut (Video, u64)) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
