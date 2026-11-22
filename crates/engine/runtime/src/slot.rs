use std::sync::{Mutex, MutexGuard, PoisonError};

/// A bounded single-value handoff: a new value replaces an untaken one.
///
/// The session offers previews and configuration here. Nothing queues, so a
/// slow consumer only ever sees the latest value. (Push-style sources use
/// [`OwnerThread`](prysm_ports::OwnerThread), which hands frames over the
/// same way.)
#[derive(Debug, Default)]
pub(crate) struct LatestSlot<T> {
    value: Mutex<Option<T>>,
}

impl<T> LatestSlot<T> {
    pub(crate) fn new() -> Self {
        Self {
            value: Mutex::new(None),
        }
    }

    /// Store `value` in place of an untaken one.
    pub(crate) fn offer(&self, value: T) {
        *self.lock() = Some(value);
    }

    pub(crate) fn take(&self) -> Option<T> {
        self.lock().take()
    }

    /// The current value without taking it, for observers that only render.
    pub(crate) fn peek(&self) -> Option<T>
    where
        T: Clone,
    {
        self.lock().clone()
    }

    // A panicking holder cannot leave an `Option` half-written.
    fn lock(&self) -> MutexGuard<'_, Option<T>> {
        self.value.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_value_wins() {
        let slot = LatestSlot::new();
        slot.offer(1);
        slot.offer(2);
        assert_eq!(slot.peek(), Some(2));
        assert_eq!(slot.take(), Some(2));
        assert_eq!(slot.take(), None);
    }
}
