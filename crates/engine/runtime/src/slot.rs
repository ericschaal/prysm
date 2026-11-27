use std::sync::{Mutex, MutexGuard, PoisonError};

/// A single pending value. Offering a newer one replaces it, and taking it
/// returns ownership after releasing the lock.
#[derive(Debug, Default)]
pub(super) struct LatestSlot<T> {
    value: Mutex<Option<T>>,
}

impl<T> LatestSlot<T> {
    pub(super) fn offer(&self, value: T) {
        *self.lock() = Some(value);
    }

    pub(super) fn take(&self) -> Option<T> {
        self.lock().take()
    }

    fn lock(&self) -> MutexGuard<'_, Option<T>> {
        self.value.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
