//! Restarting a system call that asks for it; safe and testable on any host.
use std::io;

/// Calls made before a call that keeps asking to be restarted fails.
const ATTEMPTS: usize = 100;

/// Run `call` again while it fails with `EINTR` or `EAGAIN`, as the kernel's
/// dma-buf documentation requires for `DMA_BUF_IOCTL_SYNC` ("CPU Access to
/// DMA Buffer Objects"), at most [`ATTEMPTS`] times in all.
///
/// # Errors
/// The first other error, or the last one after [`ATTEMPTS`] calls.
pub(crate) fn restarting(mut call: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut attempts = 1;
    loop {
        match call() {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) && attempts < ATTEMPTS =>
            {
                attempts += 1;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use io::ErrorKind::{Interrupted, InvalidInput, WouldBlock};

    /// Restart a call that fails with `errors` in turn, then succeeds; the
    /// result and how many calls ran.
    fn run(errors: &[io::ErrorKind]) -> (io::Result<()>, usize) {
        let mut calls = 0;
        let result = restarting(|| {
            calls += 1;
            errors
                .get(calls - 1)
                .map_or(Ok(()), |kind| Err(io::Error::from(*kind)))
        });
        (result, calls)
    }

    #[test]
    fn interrupted_and_would_block_calls_are_restarted() {
        let (result, calls) = run(&[Interrupted, WouldBlock, Interrupted]);
        result.unwrap();
        assert_eq!(calls, 4);
    }

    #[test]
    fn other_errors_are_returned_at_once() {
        let (result, calls) = run(&[Interrupted, InvalidInput]);
        assert_eq!(result.unwrap_err().kind(), InvalidInput);
        assert_eq!(calls, 2);
    }

    #[test]
    fn restarts_are_bounded_and_keep_the_last_error() {
        let (result, calls) = run(&[WouldBlock; ATTEMPTS + 1]);
        assert_eq!(result.unwrap_err().kind(), WouldBlock);
        assert_eq!(calls, ATTEMPTS);
    }
}
