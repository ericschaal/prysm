//! The uplink's one classification: what it heard after a failed
//! connection attempt or a lost connection.
use prysm_capture_remote::{Close, RemoteSettings, SendError, WireError};
use prysm_ports::{Error, chain};

/// What failed on the uplink.
#[derive(Debug)]
pub(crate) enum Cause<'a> {
    /// A connection attempt.
    Attempt(&'a Error),
    /// The open connection with this request.
    Lost(RemoteSettings, &'a SendError),
}

/// What the uplink heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Heard {
    /// The receiver is unavailable, and why: its host is not lighting, or
    /// it is unreachable.
    Absent(String),
    /// The receiver stopped the stream.
    Stopped,
    /// The receiver's source failed with this stream, and how it said so.
    Failed(String),
    /// Prysm speaks another capture stream version, and which.
    OtherVersion(String),
    /// This end refused the request (its encoder failed), and why.
    Refused {
        request: RemoteSettings,
        reason: String,
    },
    /// Another sender replaced this one at the receiver, and how it was
    /// told.
    Replaced(String),
}

/// The uplink's rules, one row per cause. Another version counts only at
/// the upgrade, a close code only on an open connection; otherwise each is
/// waited out like an absent receiver.
pub(crate) fn heard(cause: Cause<'_>) -> Heard {
    use WireError as W;
    let (open, error) = match cause {
        Cause::Lost(request, SendError::Refused(error)) => {
            return Heard::Refused {
                request,
                reason: chain(error),
            };
        }
        Cause::Lost(_, SendError::Connection(error)) => (true, error),
        Cause::Attempt(error) => (false, error),
    };
    let waiting = || format!("waiting for the receiver: {}", chain(error));
    match (open, error.cause::<WireError>()) {
        (false, Some(W::Version { .. })) => Heard::OtherVersion(waiting()),
        (true, Some(W::Closed(Some(Close::Stopped)))) => Heard::Stopped,
        (true, Some(W::Closed(Some(Close::Replaced)))) => Heard::Replaced(chain(error)),
        (true, Some(W::Closed(Some(Close::Failed)))) => Heard::Failed(chain(error)),
        (false, Some(W::Closed(Some(_))))
        | (true, Some(W::Version { .. }))
        | (
            _,
            None
            | Some(
                W::Unavailable(_)
                | W::Untrusted(_)
                | W::NoRequest
                | W::Json(_)
                | W::Request(_)
                | W::Unexpected
                | W::Refused(_)
                | W::Closed(None)
                | W::TextTooLarge(_)
                | W::Short(_)
                | W::FormatCode { .. }
                | W::Format(_)
                | W::NotH264(_)
                | W::Dimensions { .. }
                | W::TooLarge(_)
                | W::Empty
                | W::Length { .. }
                | W::Geometry(_),
            ),
        ) => Heard::Absent(waiting()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{FrameError, PixelFormat};
    use prysm_ports::{StreamEncoding, ValidationError};

    const REQUEST: RemoteSettings = RemoteSettings {
        width: 320,
        height: 180,
        fps: 30,
        include_ui: true,
        encoding: StreamEncoding::Raw,
    };

    fn absent(error: &Error) -> Heard {
        Heard::Absent(format!("waiting for the receiver: {}", chain(error)))
    }
    fn other_version(error: &Error) -> Heard {
        Heard::OtherVersion(format!("waiting for the receiver: {}", chain(error)))
    }
    fn stopped(_: &Error) -> Heard {
        Heard::Stopped
    }
    fn replaced(error: &Error) -> Heard {
        Heard::Replaced(chain(error))
    }
    fn failed(error: &Error) -> Heard {
        Heard::Failed(chain(error))
    }

    /// One row per `WireError` variant, and per close code: what an attempt
    /// and an open connection that fail with it hear.
    #[test]
    fn each_cause_is_heard_once() {
        use WireError as W;
        type Rule = fn(&Error) -> Heard;
        let closed = |close| Some(W::Closed(close));
        let pixels = PixelFormat::RGB24;
        #[rustfmt::skip]
        let rows: [(Option<WireError>, Rule, Rule); 24] = [
            (None, absent, absent),
            (Some(W::Version { ours: 6, theirs: Some(9) }), other_version, absent),
            (Some(W::Unavailable(503)), absent, absent),
            (Some(W::Unavailable(404)), absent, absent),
            (Some(W::Untrusted("x".into())), absent, absent),
            (Some(W::NoRequest), absent, absent),
            (Some(W::Json("x".into())), absent, absent),
            (Some(W::Request(ValidationError::new("fps", "x"))), absent, absent),
            (Some(W::Unexpected), absent, absent),
            (Some(W::Refused("x".into())), absent, absent),
            (closed(None), absent, absent),
            (closed(Some(Close::Stopped)), absent, stopped),
            (closed(Some(Close::Replaced)), absent, replaced),
            (closed(Some(Close::Failed)), absent, failed),
            (Some(W::TextTooLarge(1)), absent, absent),
            (Some(W::Short(1)), absent, absent),
            (Some(W::FormatCode { format: 9, range: 0, matrix: 0 }), absent, absent),
            (Some(W::Format(pixels)), absent, absent),
            (Some(W::NotH264(pixels)), absent, absent),
            (Some(W::Dimensions { width: 0, height: 0 }), absent, absent),
            (Some(W::TooLarge(1)), absent, absent),
            (Some(W::Empty), absent, absent),
            (Some(W::Length { expected: 2, actual: 1 }), absent, absent),
            (Some(W::Geometry(FrameError::SizeOverflow)), absent, absent),
        ];
        for (wire, attempt, open) in rows {
            let error = match wire.clone() {
                Some(wire) => Error::adapter("the stream failed", wire),
                None => Error::other("cannot connect: refused"),
            };
            assert_eq!(heard(Cause::Attempt(&error)), attempt(&error), "{wire:?}");
            let lost = SendError::Connection(error.clone());
            assert_eq!(heard(Cause::Lost(REQUEST, &lost)), open(&error), "{wire:?}");
        }
    }

    /// A refusal is this end's, with the request it refused.
    #[test]
    fn a_refusal_keeps_its_request() {
        let refused = SendError::Refused(Error::other("the encoder is busy"));
        assert_eq!(
            heard(Cause::Lost(REQUEST, &refused)),
            Heard::Refused {
                request: REQUEST,
                reason: "the encoder is busy".into(),
            }
        );
    }

    /// The texts the link shows.
    #[test]
    fn heard_texts_name_the_cause() {
        let closed = |close| {
            SendError::Connection(Error::adapter(
                "the stream failed",
                WireError::Closed(close),
            ))
        };
        assert_eq!(
            heard(Cause::Lost(REQUEST, &closed(Some(Close::Replaced)))),
            Heard::Replaced("the stream failed: another sender replaced this one at Prysm".into())
        );
        assert_eq!(
            heard(Cause::Lost(REQUEST, &closed(None))),
            Heard::Absent(
                "waiting for the receiver: the stream failed: the peer closed the stream".into()
            )
        );
    }
}
