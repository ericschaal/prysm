//! The crate's one classification of failures: what the output does after
//! each cause. Nothing here waits, retries or keeps state.
use std::io::{self, ErrorKind};

/// What a held output does after a colour frame's send failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Send {
    /// The frame is dropped and `write` succeeds: the datagram could not
    /// leave now, and the next frame is the next try. There is nothing to
    /// wait for and nothing to keep.
    FrameDropped,
    /// `write` fails with the error (fail fast): no other cause has a
    /// reason to clear by the next frame.
    OutputFails,
}

/// What a send error from the DDP socket means, one row per cause.
pub(crate) fn send(error: &io::Error) -> Send {
    match error.kind() {
        // The socket's send buffer is full; it drains on its own.
        ErrorKind::WouldBlock
        // No route to the device or its network now (address resolution
        // failed, a link is down or coming up); the next send asks again.
        | ErrorKind::HostUnreachable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::NetworkDown => Send::FrameDropped,
        _ => without_kind(error),
    }
}

/// The causes std gives no kind of its own, by their OS error numbers.
#[cfg(unix)]
fn without_kind(error: &io::Error) -> Send {
    match error.raw_os_error() {
        // The interface's queue is full; it drains on its own.
        Some(libc::ENOBUFS)
        // The device does not answer address resolution (macOS); the next
        // send asks again.
        | Some(libc::EHOSTDOWN) => Send::FrameDropped,
        _ => Send::OutputFails,
    }
}

#[cfg(not(unix))]
fn without_kind(_: &io::Error) -> Send {
    Send::OutputFails
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn each_send_error_has_its_row() {
        let kind = |kind: ErrorKind| io::Error::from(kind);
        let os = io::Error::from_raw_os_error;
        let rows = [
            (kind(ErrorKind::WouldBlock), Send::FrameDropped),
            (os(libc::EAGAIN), Send::FrameDropped),
            (os(libc::ENOBUFS), Send::FrameDropped),
            (kind(ErrorKind::HostUnreachable), Send::FrameDropped),
            (os(libc::EHOSTUNREACH), Send::FrameDropped),
            (os(libc::EHOSTDOWN), Send::FrameDropped),
            (kind(ErrorKind::NetworkUnreachable), Send::FrameDropped),
            (os(libc::ENETUNREACH), Send::FrameDropped),
            (kind(ErrorKind::NetworkDown), Send::FrameDropped),
            (os(libc::ENETDOWN), Send::FrameDropped),
            // Anything else.
            (kind(ErrorKind::PermissionDenied), Send::OutputFails),
            (os(libc::EACCES), Send::OutputFails),
            (kind(ErrorKind::InvalidInput), Send::OutputFails),
            (os(libc::EINVAL), Send::OutputFails),
        ];
        for (error, expected) in rows {
            assert_eq!(send(&error), expected, "{error:?}");
        }
    }
}
