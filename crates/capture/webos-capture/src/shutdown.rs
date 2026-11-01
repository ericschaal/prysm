use anyhow::Result;
use nix::sys::signal::{SigSet, Signal};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Call before starting other threads so native capture inherits blocked signals.
pub fn listen() -> Result<(Arc<AtomicBool>, JoinHandle<()>)> {
    let mut signals = SigSet::empty();
    signals.add(Signal::SIGINT);
    signals.add(Signal::SIGTERM);
    // Interrupting webOS video preprocessing with a signal handler can corrupt its caller.
    signals.thread_block()?;
    let stop = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::clone(&stop);
    let waiter = thread::Builder::new()
        .name("shutdown".into())
        .spawn(move || {
            if let Err(error) = signals.wait() {
                tracing::error!(%error, "cannot wait for shutdown signal");
            }
            shutdown.store(true, Ordering::Relaxed);
        })?;
    Ok((stop, waiter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::thread::JoinHandleExt;

    #[test]
    fn shutdown_is_consumed_without_interrupting_the_capture_thread() {
        for signal in [Signal::SIGINT, Signal::SIGTERM] {
            thread::spawn(move || {
                let (stop, waiter) = listen().unwrap();
                let mask = SigSet::thread_get_mask().unwrap();
                assert!(mask.contains(Signal::SIGINT));
                assert!(mask.contains(Signal::SIGTERM));
                thread::spawn(|| {
                    let inherited = SigSet::thread_get_mask().unwrap();
                    assert!(inherited.contains(Signal::SIGINT));
                    assert!(inherited.contains(Signal::SIGTERM));
                })
                .join()
                .unwrap();
                assert!(!stop.load(Ordering::Relaxed));
                nix::sys::pthread::pthread_kill(waiter.as_pthread_t(), signal).unwrap();
                waiter.join().unwrap();
                assert!(stop.load(Ordering::Relaxed));
            })
            .join()
            .unwrap();
        }
    }
}
