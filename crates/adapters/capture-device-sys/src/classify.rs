//! The one reading of V4L2 errors: what an errno from one of capture's
//! operations means. The same errno means different things per operation,
//! so a row names both. Built on every platform under test, so the table
//! runs everywhere.
use crate::CaptureError;
use std::io;

/// What capture asked of the device when it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
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
        (Op::Dequeue | Op::Requeue, Some(libc::EIO)) => Fault::QueueError,
        (Op::Wait, Some(libc::EINTR)) => Fault::Signalled,
        _ => Fault::Other,
    }
}

/// The capture failure `error` is as `fault`, `doing` what.
pub(crate) fn failure(fault: Fault, error: io::Error, doing: &'static str) -> CaptureError {
    match fault {
        Fault::Gone => CaptureError::Disconnected {
            details: doing.into(),
            source: Some(error),
        },
        Fault::QueueError | Fault::Signalled | Fault::Other => {
            anyhow::Error::new(error).context(doing).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_and_errno_has_one_meaning() {
        use Fault::{Gone, Other, QueueError, Signalled};
        use Op::{Dequeue, Release, Requeue, Start, Stop, Wait};
        for (op, errno, fault) in [
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
            (Start, libc::EBUSY, Other),
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
        for fault in [Fault::QueueError, Fault::Signalled, Fault::Other] {
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
