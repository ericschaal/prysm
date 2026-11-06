//! Typed persistence; the application owns validation and revision ordering.
use crate::ControlError;
use std::future::Future;

pub trait SettingsRepository<S>: Send + 'static {
    fn load(&self) -> impl Future<Output = Result<Option<S>, ControlError>> + Send;
    /// Commit atomically before reporting success. Ordinary errors preserve the previous
    /// value; RecoveryRequired means commit/durability is uncertain and requires recovery.
    /// Adapters must retain started I/O independently of a cancelled awaiting caller.
    fn save(&self, settings: S) -> impl Future<Output = Result<(), ControlError>> + Send;
}
