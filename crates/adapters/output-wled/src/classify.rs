//! The crate's one classification of failures: what the output does after
//! each cause, and what each failure escalates as once it leaves the crate.
//! Nothing here waits, retries or keeps state.
use crate::http::{Malformed, RequestError};
use prysm_ports::{CallError, Escalation, Fix};
use std::io::{self, ErrorKind};

/// What failed, for an error the crate builds.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Cause<'a> {
    /// A request to the JSON API.
    Request(&'a RequestError),
    /// Resolving the configured host name.
    Resolve(&'a CallError),
    /// The host name resolved only to an address that is not unicast.
    NotUnicast,
    /// Opening the local DDP socket.
    Socket,
    /// A colour frame's send whose cause has no row in [`send`].
    Send,
}

/// What a failure escalates as, one row per cause.
pub(crate) fn escalation(cause: Cause<'_>) -> Escalation {
    use RequestError as R;
    match cause {
        // Nothing answered: the device may be off or away from the network,
        // which nothing in Prysm changes and nothing announces the end of.
        // A local socket the connect could not create comes here too: the
        // connect's error does not say which it was.
        Cause::Request(R::Connect(CallError::Failed(_)) | R::Io(_) | R::TimedOut(_))
        | Cause::Request(R::Malformed(Malformed::Closed))
        // The lookup's error does not tell a name nobody answers for from a
        // lookup thread that could not run.
        | Cause::Resolve(CallError::Failed(_)) => Escalation::Unanswered,
        // Something answered that is not a WLED, or not where the settings
        // point: only other settings change that.
        Cause::Request(R::Status(404) | R::TooLarge(_) | R::Json(_))
        | Cause::Request(R::Malformed(
            Malformed::HeaderNotUtf8
            | Malformed::StatusLine
            | Malformed::ContentLength
            | Malformed::ChunkSize,
        ))
        | Cause::NotUnicast => Escalation::Ask(Fix::ChangeSettings),
        // A WLED that refused the request; a cut, which the one who cut it
        // follows up; a local socket; a send with no reason to clear: no
        // cause here has an argument that it passes.
        Cause::Request(
            R::Status(_)
            | R::Rejected(_)
            | R::Unacknowledged
            | R::Stopped(_)
            | R::Connect(CallError::Stopped(_)),
        )
        | Cause::Resolve(CallError::Stopped(_))
        | Cause::Socket
        | Cause::Send => Escalation::FAIL_FAST,
    }
}

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

    #[test]
    fn each_failure_escalates_by_its_row() {
        use Malformed as M;
        use RequestError as R;
        let unanswered = Escalation::Unanswered;
        let settings = Escalation::Ask(Fix::ChangeSettings);
        let fail = Escalation::FAIL_FAST;
        let failed = || CallError::Failed(ErrorKind::ConnectionRefused.into());
        let stopped = || CallError::Stopped(prysm_ports::Stopping);
        let json = || serde_json::from_str::<serde_json::Value>("x").unwrap_err();
        let requests = [
            (R::Connect(failed()), unanswered),
            (R::Io(ErrorKind::ConnectionReset.into()), unanswered),
            (R::TimedOut(std::time::Duration::from_secs(1)), unanswered),
            (R::Malformed(M::Closed), unanswered),
            (R::Status(404), settings),
            (R::TooLarge(1024), settings),
            (R::Json(json()), settings),
            (R::Malformed(M::HeaderNotUtf8), settings),
            (R::Malformed(M::StatusLine), settings),
            (R::Malformed(M::ContentLength), settings),
            (R::Malformed(M::ChunkSize), settings),
            (R::Status(500), fail),
            (R::Status(301), fail),
            (R::Rejected("9".into()), fail),
            (R::Unacknowledged, fail),
            (R::Stopped(prysm_ports::Stopping), fail),
            (R::Connect(stopped()), fail),
        ];
        for (error, expected) in requests {
            assert_eq!(escalation(Cause::Request(&error)), expected, "{error:?}");
        }
        let (failed, stopped) = (failed(), stopped());
        let others = [
            (Cause::Resolve(&failed), unanswered),
            (Cause::Resolve(&stopped), fail),
            (Cause::NotUnicast, settings),
            (Cause::Socket, fail),
            (Cause::Send, fail),
        ];
        for (cause, expected) in others {
            assert_eq!(escalation(cause), expected, "{cause:?}");
        }
    }
}
