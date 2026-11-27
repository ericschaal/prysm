//! The uplink's one classification: what it heard after a failed
//! connection attempt or a lost connection, or that it ends.
use super::Disconnected;
use prysm_capture_remote::{Close, RemoteSettings, SendError, WireError};
use prysm_ports::{Error, Escalation, Fix, chain};

/// What failed on the uplink.
#[derive(Debug)]
pub(crate) enum Cause<'a> {
    /// A connection attempt.
    Attempt(&'a Error),
    /// The open connection with this request.
    Lost(RemoteSettings, &'a SendError),
}

/// The uplink's rules, one row per cause. `Err`: waiting cannot help, so
/// the uplink ends and the worker fails with this escalation; the TV asks
/// the person. Another sender replaced this one (it takes the stream until
/// its own user stops it, so two TVs never take it from each other in
/// turn), a wrong address (`404`: no portal there, or an older Prysm), a
/// certificate this end cannot trust, or a receiver that speaks the
/// protocol wrongly, which the same Prysm repeats. Another version counts
/// only at the upgrade, a stop only on an open connection; everything else
/// is waited out like an absent receiver.
pub(crate) fn heard(cause: Cause<'_>) -> Result<Disconnected, Escalation> {
    use WireError as W;
    let (open, error) = match cause {
        Cause::Lost(request, SendError::Refused(error)) => {
            return Ok(Disconnected::Refused {
                request,
                reason: chain(error),
            });
        }
        Cause::Lost(_, SendError::Connection(error)) => (true, error),
        Cause::Attempt(error) => (false, error),
    };
    let waiting = || format!("waiting for the receiver: {}", chain(error));
    Ok(match (open, error.cause::<WireError>()) {
        (_, Some(W::Closed(Some(Close::Replaced)))) => return Err(Escalation::FAIL_FAST),
        (_, Some(W::Unavailable(401 | 403 | 404) | W::Untrusted(_))) => {
            return Err(Escalation::Ask(Fix::ChangeSettings));
        }
        (
            _,
            Some(W::Json(_) | W::Request(_) | W::Refused(_) | W::Unexpected | W::TextTooLarge(_)),
        ) => {
            return Err(Escalation::FAIL_FAST);
        }
        (false, Some(W::Version { .. })) => Disconnected::OtherVersion(waiting()),
        (true, Some(W::Closed(Some(Close::Stopped)))) => Disconnected::Stopped,
        (_, Some(W::Closed(Some(Close::Failed)))) => Disconnected::Failed(chain(error)),
        (false, Some(W::Closed(Some(Close::Stopped))))
        | (true, Some(W::Version { .. }))
        | (
            _,
            None
            | Some(
                W::Unavailable(_)
                | W::NoRequest
                | W::Closed(None)
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
        ) => Disconnected::Absent(waiting()),
    })
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

    type Outcome = Result<Disconnected, Escalation>;

    fn absent(error: &Error) -> Outcome {
        Ok(Disconnected::Absent(format!(
            "waiting for the receiver: {}",
            chain(error)
        )))
    }
    fn other_version(error: &Error) -> Outcome {
        Ok(Disconnected::OtherVersion(format!(
            "waiting for the receiver: {}",
            chain(error)
        )))
    }
    fn stopped(_: &Error) -> Outcome {
        Ok(Disconnected::Stopped)
    }
    fn failed(error: &Error) -> Outcome {
        Ok(Disconnected::Failed(chain(error)))
    }
    fn fail_fast(_: &Error) -> Outcome {
        Err(Escalation::FAIL_FAST)
    }
    fn change_settings(_: &Error) -> Outcome {
        Err(Escalation::Ask(Fix::ChangeSettings))
    }

    /// One row per `WireError` variant, and per close code: what an attempt
    /// and an open connection that fail with it hear.
    #[test]
    fn each_cause_is_heard_once() {
        use WireError as W;
        type Rule = fn(&Error) -> Outcome;
        let closed = |close| Some(W::Closed(close));
        let pixels = PixelFormat::RGB24;
        #[rustfmt::skip]
        let rows: [(Option<WireError>, Rule, Rule); 27] = [
            (None, absent, absent),
            (Some(W::Version { ours: 6, theirs: Some(9) }), other_version, absent),
            (Some(W::Unavailable(503)), absent, absent),
            (Some(W::Unavailable(500)), absent, absent),
            (Some(W::Unavailable(401)), change_settings, change_settings),
            (Some(W::Unavailable(403)), change_settings, change_settings),
            (Some(W::Unavailable(404)), change_settings, change_settings),
            (Some(W::Untrusted("x".into())), change_settings, change_settings),
            (Some(W::NoRequest), absent, absent),
            (Some(W::Json("x".into())), fail_fast, fail_fast),
            (Some(W::Request(ValidationError::new("fps", "x"))), fail_fast, fail_fast),
            (Some(W::Unexpected), fail_fast, fail_fast),
            (Some(W::Refused("x".into())), fail_fast, fail_fast),
            (closed(None), absent, absent),
            (closed(Some(Close::Stopped)), absent, stopped),
            (closed(Some(Close::Replaced)), fail_fast, fail_fast),
            (closed(Some(Close::Failed)), failed, failed),
            (Some(W::TextTooLarge(1)), fail_fast, fail_fast),
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
            Ok(Disconnected::Refused {
                request: REQUEST,
                reason: "the encoder is busy".into(),
            })
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
            heard(Cause::Lost(REQUEST, &closed(Some(Close::Failed)))),
            Ok(Disconnected::Failed(
                "the stream failed: Prysm stopped receiving after a failure, which it shows".into()
            ))
        );
        assert_eq!(
            heard(Cause::Lost(REQUEST, &closed(None))),
            Ok(Disconnected::Absent(
                "waiting for the receiver: the stream failed: the peer closed the stream".into()
            ))
        );
    }
}
