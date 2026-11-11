//! Supported presentation policy; startup is Off and recovery requires an explicit command.
use prysm_core::EdgeColors;

#[derive(Default)]
pub struct ApplicationPolicy {
    /// Colors held while an active session waits for capture availability.
    pub waiting_colors: EdgeColors,
}
