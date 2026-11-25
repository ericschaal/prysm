//! The one reading of V4L2 errors: what an errno from one of capture's
//! operations means. The same errno means different things per operation,
//! so a row names both. Built on every platform under test, so the table
//! runs everywhere.
use crate::CaptureError;
use std::io;

/// What capture asked of the device when it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    /// Opening the device node.
    Open,
    /// `VIDIOC_S_FMT`, then reading back what the driver set.
    SetFormat,
    /// Requesting, querying and mapping the buffers.
    Allocate,
    /// Polling for a filled buffer.
    Wait,
    /// `VIDIOC_DQBUF`.
    Dequeue,
    /// `VIDIOC_QBUF` of a buffer just read.
    Requeue,
    /// Queueing every buffer, then `VIDIOC_STREAMON`.
    Start,
    /// `VIDIOC_STREAMOFF`.
    Stop,
    /// Stopping, unmapping and freeing the buffers.
    Release,
}

/// What an error means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    /// ENODEV: the node is unregistered (unplugged or reset). The V4L2 core
    /// then answers every ioctl with ENODEV without calling the driver, and
    /// unregistering stops streaming and frees the queue's buffers (at the
    /// latest when the fd closes), so a release has nothing left to free.
    Gone,
    /// EBUSY: another program holds the device or its buffers.
    Busy,
    /// EACCES or EPERM at open: the process may not open the node.
    Denied,
    /// ENOENT at open: no node at that path.
    Missing,
    /// EINVAL from S_FMT: some drivers refuse a format they do not support
    /// this way, so the next format is tried.
    Rejected,
    /// EIO from DQBUF or QBUF, which may be temporary: V4L2 also uses it
    /// for problems like signal loss, so the stream restarts
    /// ([`crate::health`]).
    QueueError,
    /// EINTR from poll: a signal handler ran on this thread. poll(2) is
    /// never restarted, so the wait goes on for what is left.
    Signalled,
    /// Anything else. EIO, EPROTO or ESHUTDOWN elsewhere can come from a
    /// device still registered, whose buffers may still be held.
    Other,
}

/// The table. Errnos are an open set: those without a row are
/// [`Fault::Other`].
pub(crate) fn classify(op: Op, error: &io::Error) -> Fault {
    match (op, error.raw_os_error()) {
        (_, Some(libc::ENODEV)) => Fault::Gone,
        // A release that fails leaves the device held, whatever the errno.
        (Op::Release, _) => Fault::Other,
        (_, Some(libc::EBUSY)) => Fault::Busy,
        (Op::Open, Some(libc::EACCES | libc::EPERM)) => Fault::Denied,
        (Op::Open, Some(libc::ENOENT)) => Fault::Missing,
        (Op::SetFormat, Some(libc::EINVAL)) => Fault::Rejected,
        (Op::Dequeue | Op::Requeue, Some(libc::EIO)) => Fault::QueueError,
        (Op::Wait, Some(libc::EINTR)) => Fault::Signalled,
        _ => Fault::Other,
    }
}

/// The capture failure `error` is as `fault`, `doing` what. Open handles
/// [`Fault::Denied`] and [`Fault::Missing`] itself, since they name the path.
pub(crate) fn failure(fault: Fault, error: io::Error, doing: impl Into<String>) -> CaptureError {
    let doing = doing.into();
    match fault {
        Fault::Gone => CaptureError::Disconnected {
            details: doing,
            source: Some(error),
        },
        Fault::Busy => CaptureError::Busy {
            details: doing,
            source: error,
        },
        Fault::Denied
        | Fault::Missing
        | Fault::Rejected
        | Fault::QueueError
        | Fault::Signalled
        | Fault::Other => anyhow::Error::new(error).context(doing).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_and_errno_has_one_meaning() {
        use Fault::{Busy, Denied, Gone, Missing, Other, QueueError, Rejected, Signalled};
        use Op::{Allocate, Dequeue, Open, Release, Requeue, SetFormat, Start, Stop, Wait};
        for (op, errno, fault) in [
            (Open, libc::ENOENT, Missing),
            (Open, libc::EACCES, Denied),
            (Open, libc::EPERM, Denied),
            (Open, libc::EBUSY, Busy),
            (Open, libc::ENODEV, Gone),
            (Open, libc::EINVAL, Other),
            (SetFormat, libc::EINVAL, Rejected),
            (SetFormat, libc::EBUSY, Busy),
            (SetFormat, libc::EACCES, Other),
            (SetFormat, libc::ENOENT, Other),
            (SetFormat, libc::ENODEV, Gone),
            (Allocate, libc::EBUSY, Busy),
            (Allocate, libc::EINVAL, Other),
            (Allocate, libc::ENOMEM, Other),
            (Dequeue, libc::EBUSY, Busy),
            (Wait, libc::ENODEV, Gone),
            (Dequeue, libc::ENODEV, Gone),
            (Requeue, libc::ENODEV, Gone),
            (Start, libc::ENODEV, Gone),
            (Stop, libc::ENODEV, Gone),
            (Release, libc::ENODEV, Gone),
            (Dequeue, libc::EIO, QueueError),
            (Requeue, libc::EIO, QueueError),
            (Wait, libc::EIO, Other),
            (Start, libc::EIO, Other),
            (Stop, libc::EIO, Other),
            (Release, libc::EIO, Other),
            (Wait, libc::EINTR, Signalled),
            (Dequeue, libc::EINTR, Other),
            (Wait, libc::EBADF, Other),
            (Dequeue, libc::EINVAL, Other),
            (Requeue, libc::EINVAL, Other),
            (Dequeue, libc::EPIPE, Other),
            (Start, libc::EBUSY, Busy),
            (Release, libc::EBUSY, Other),
        ] {
            let error = io::Error::from_raw_os_error(errno);
            assert_eq!(classify(op, &error), fault, "{op:?} {errno}");
        }
        assert_eq!(classify(Dequeue, &io::Error::other("no errno")), Other);
    }

    #[test]
    fn a_device_gone_is_a_disconnection_and_the_rest_backend_errors() {
        let gone = io::Error::from_raw_os_error(libc::ENODEV);
        let CaptureError::Disconnected { details, source } =
            failure(Fault::Gone, gone, "error capturing frame")
        else {
            panic!("a device gone is a disconnection");
        };
        assert_eq!(details, "error capturing frame");
        assert_eq!(source.unwrap().raw_os_error(), Some(libc::ENODEV));
        let busy = io::Error::from_raw_os_error(libc::EBUSY);
        let CaptureError::Busy { details, source } =
            failure(Fault::Busy, busy, "failed to set capture format")
        else {
            panic!("a busy device is typed");
        };
        assert_eq!(details, "failed to set capture format");
        assert_eq!(source.raw_os_error(), Some(libc::EBUSY));
        for fault in [
            Fault::Denied,
            Fault::Missing,
            Fault::Rejected,
            Fault::QueueError,
            Fault::Signalled,
            Fault::Other,
        ] {
            let error = failure(fault, io::Error::from_raw_os_error(libc::EIO), "doing");
            let CaptureError::Backend(error) = error else {
                panic!("{fault:?} is a backend error");
            };
            assert_eq!(error.to_string(), "doing");
            let cause = error.downcast_ref::<io::Error>().unwrap();
            assert_eq!(cause.raw_os_error(), Some(libc::EIO));
        }
    }
}
