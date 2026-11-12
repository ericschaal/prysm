use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A bounded single-value handoff: a new value replaces an untaken one.
///
/// Push-style sources keep their newest frame here and take it in `next_frame`;
/// the session offers previews here. Nothing queues, so a slow consumer only
/// ever sees the latest value.
#[derive(Debug, Default)]
pub struct LatestSlot<T> {
    value: Mutex<Option<T>>,
    filled: Condvar,
}

impl<T> LatestSlot<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            value: Mutex::new(None),
            filled: Condvar::new(),
        }
    }

    /// Store `value`, returning the untaken value it replaced.
    pub fn offer(&self, value: T) -> Option<T> {
        let replaced = self.lock().replace(value);
        self.filled.notify_one();
        replaced
    }

    pub fn take(&self) -> Option<T> {
        self.lock().take()
    }

    /// Wait up to `timeout` for a value and take it.
    pub fn take_timeout(&self, timeout: Duration) -> Option<T> {
        let deadline = Instant::now() + timeout;
        let mut value = self.lock();
        loop {
            if let Some(taken) = value.take() {
                return Some(taken);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            value = self
                .filled
                .wait_timeout(value, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The current value without taking it, for observers that only render.
    pub fn peek(&self) -> Option<T>
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
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn newest_value_wins() {
        let slot = LatestSlot::new();
        assert_eq!(slot.offer(1), None);
        assert_eq!(slot.offer(2), Some(1));
        assert_eq!(slot.peek(), Some(2));
        assert_eq!(slot.take(), Some(2));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn take_timeout_returns_none_when_empty() {
        let slot = LatestSlot::<u8>::new();
        let started = Instant::now();
        assert_eq!(slot.take_timeout(Duration::from_millis(30)), None);
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn take_timeout_wakes_on_offer() {
        let slot = Arc::new(LatestSlot::new());
        let producer = {
            let slot = slot.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                slot.offer(7);
            })
        };
        assert_eq!(slot.take_timeout(Duration::from_secs(5)), Some(7));
        producer.join().unwrap();
    }
}
