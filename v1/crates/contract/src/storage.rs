//! Typed persistence; the application owns validation and revision ordering.
use crate::ControlError;
use std::future::Future;

pub trait SettingsRepository<S>: Send + 'static {
    fn load(&self) -> impl Future<Output = Result<Option<S>, ControlError>> + Send;
    /// Commit atomically before reporting success. Preserve the previous value on failure.
    fn save(&self, settings: S) -> impl Future<Output = Result<(), ControlError>> + Send;
}
