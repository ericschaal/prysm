//! Data-parallel loops inside one blocking `process` call.
//!
//! A deliberate, contained exception to "concurrency lives in
//! `prysm-runtime`" (decision: `docs/developers/decisions/2026-11-14-parallel-sampling.md`):
//! a loop over independent items is split into chunks that run on a small
//! dedicated pool, never rayon's global one, and every chunk has finished
//! before the call returns. Nothing is shared between frames or chunks.

use rayon::ThreadPool;
use rayon::prelude::*;
use std::num::NonZeroUsize;

/// Chunks per pool thread, so a thread slowed by other work on the TV does
/// not hold the others up for a whole share.
const CHUNKS_PER_THREAD: usize = 4;

/// The threads a processor works with: the caller alone, or a dedicated pool.
#[derive(Debug, Default)]
pub(crate) struct Workers {
    /// `None` runs every loop on the caller's thread, exactly as one loop.
    pool: Option<ThreadPool>,
}

impl Workers {
    /// One thread is the caller alone: no pool and no rayon overhead. A pool
    /// that cannot start its threads falls back to the caller alone.
    pub(crate) fn new(threads: NonZeroUsize) -> Self {
        if threads.get() == 1 {
            return Self::default();
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.get())
            .thread_name(|index| format!("prysm-sampler-{index}"))
            .build();
        match pool {
            Ok(pool) => Self { pool: Some(pool) },
            Err(error) => {
                tracing::warn!("Sampling on one thread; the sampler pool did not start: {error}");
                Self::default()
            }
        }
    }

    /// The automatic choice: half the cores, so the rest of the system (the
    /// TV's interface and video) keeps the other half; at most three workers,
    /// at least one. On the four-core TV two workers gave most of the gain.
    pub(crate) fn automatic_threads() -> NonZeroUsize {
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        NonZeroUsize::new((cores / 2).clamp(1, 3)).unwrap_or(NonZeroUsize::MIN)
    }

    /// Calls `work(offset, chunk)` on consecutive chunks of `items` that
    /// together cover it, each chunk at least `min_len` long (except the
    /// last), where `offset` is the chunk's first index in `items`. Returns
    /// once every chunk is done. Without a pool, or with too few items for
    /// two chunks, this is the single call `work(0, items)`.
    pub(crate) fn for_each_chunk<T, F>(&self, items: &mut [T], min_len: usize, work: F)
    where
        T: Send,
        F: Fn(usize, &mut [T]) + Sync,
    {
        let Some(pool) = self
            .pool
            .as_ref()
            .filter(|_| items.len() >= 2 * min_len.max(1))
        else {
            work(0, items);
            return;
        };
        let chunks = pool.current_num_threads() * CHUNKS_PER_THREAD;
        let size = items.len().div_ceil(chunks).max(min_len).max(1);
        pool.install(|| {
            items
                .par_chunks_mut(size)
                .enumerate()
                .for_each(|(index, chunk)| work(index * size, chunk));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_every_item_once_at_their_offsets() {
        for threads in [1, 3] {
            let workers = Workers::new(NonZeroUsize::new(threads).unwrap());
            for (len, min_len) in [(0, 8), (1, 8), (15, 8), (16, 8), (1000, 8), (1001, 1)] {
                let mut items = vec![usize::MAX; len];
                workers.for_each_chunk(&mut items, min_len, |offset, chunk| {
                    for (index, item) in chunk.iter_mut().enumerate() {
                        assert_eq!(*item, usize::MAX, "written twice");
                        *item = offset + index;
                    }
                });
                assert!(items.iter().copied().eq(0..len), "{threads} threads, {len}");
            }
        }
    }

    #[test]
    fn pool_threads_are_named_and_one_thread_has_no_pool() {
        assert!(Workers::new(NonZeroUsize::MIN).pool.is_none());
        let workers = Workers::new(NonZeroUsize::new(2).unwrap());
        let mut names = vec![String::new(); 64];
        workers.for_each_chunk(&mut names, 1, |_, chunk| {
            let name = std::thread::current().name().unwrap_or_default().to_owned();
            chunk.fill(name);
        });
        assert!(names.iter().all(|name| name.starts_with("prysm-sampler-")));
    }

    #[test]
    fn automatic_choice_is_one_to_three_threads() {
        assert!((1..=3).contains(&Workers::automatic_threads().get()));
    }
}
