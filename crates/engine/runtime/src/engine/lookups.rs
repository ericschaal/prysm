//! Candidates and capture modes: blocking lookups that bypass the mailbox,
//! one at a time per key.
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use prysm_ports::{Error, Result};
use std::sync::{Arc, Mutex, PoisonError};

/// A lookup's answer, for every asker that waits for it.
pub(super) type Answer<T> = Shared<BoxFuture<'static, Result<T>>>;

/// The blocking lookups running, by what they look up. A lookup runs once
/// at a time per key: who asks while it runs waits for its answer. It keeps
/// its key until its blocking call returns, also when nobody waits any
/// more, so no client can stack up blocking threads for the same lookup.
pub(super) struct Lookups<K, T>(Arc<Mutex<InFlight<K, T>>>);

/// Each running lookup's key and answer.
type InFlight<K, T> = Vec<(K, Answer<T>)>;

impl<K, T> Default for Lookups<K, T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<K, T> Clone for Lookups<K, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K, T> Lookups<K, T>
where
    K: Clone + PartialEq + Send + 'static,
    T: Clone + Send + Sync + 'static,
{
    /// The answer of the lookup running for `key`, or of `work` started for
    /// it now on a blocking thread.
    pub(super) fn run(
        &self,
        key: K,
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Answer<T> {
        let mut running = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, answer)) = running.iter().find(|(running, _)| *running == key) {
            return answer.clone();
        }
        let ended = Ended {
            lookups: self.clone(),
            key: key.clone(),
        };
        // Started by its first poll, outside the lock, then on its thread
        // to its end whoever still waits.
        let answer = async move {
            tokio::task::spawn_blocking(move || {
                let answer = work();
                drop(ended);
                answer
            })
            .await
            .map_err(|_| Error::Panicked("blocking adapter call"))?
        }
        .boxed()
        .shared();
        running.push((key, answer.clone()));
        answer
    }
}

/// Removes a lookup that returned, on its blocking thread: the next ask
/// starts a new one.
struct Ended<K: PartialEq, T> {
    lookups: Lookups<K, T>,
    key: K,
}

impl<K: PartialEq, T> Drop for Ended<K, T> {
    fn drop(&mut self) {
        self.lookups
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(running, _)| *running != self.key);
    }
}
