//! Response encoding and the stable mapping from runtime errors to HTTP.
use bytes::Bytes;
use futures::{Stream, StreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{StatusCode, body::Frame, header};
use prysm_ports::Error;
use prysm_protocol::Failure;
use prysm_runtime::Shutdown;
use serde::Serialize;
use std::convert::Infallible;
use std::time::Duration;

/// An event stream with nothing new sends [`KEEPALIVE_COMMENT`] this often,
/// so a reader that closed is noticed (its reset fails the write) and
/// releases its stream slot. One that vanished without closing leaves
/// these comments unacknowledged; on Linux the kernel ends the connection
/// 30 s after the first of them (`TCP_USER_TIMEOUT`, set in `server`) and
/// the next write fails. Elsewhere that takes TCP's retransmission timeout.
pub(crate) const KEEPALIVE: Duration = Duration::from_secs(15);
/// An event-stream comment: ignored by readers, written like any event.
pub(crate) const KEEPALIVE_COMMENT: Bytes = Bytes::from_static(b":\n\n");

pub(crate) type Body = UnsyncBoxBody<Bytes, Infallible>;
pub(crate) type Response = hyper::Response<Body>;

pub(crate) fn bytes(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Bytes>,
) -> Response {
    let mut response = Response::new(Full::new(body.into()).boxed_unsync());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        // Asset content types are validated when the portal is constructed.
        header::HeaderValue::from_static(content_type),
    );
    response
}

/// The bundle loads only its own scripts and API, and must not be framed by
/// other sites (clickjacking). Applied to every response, assets and API alike.
pub(crate) const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; frame-ancestors 'none'";

/// The headers every response carries. `Cache-Control` is `no-store` unless
/// the response set its own: only an asset does, from its [`Caching`](crate::Caching).
pub(crate) fn secure(response: &mut Response) {
    let headers = response.headers_mut();
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(header::HeaderValue::from_static("no-store"));
    for (name, value) in [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
        (header::REFERRER_POLICY, "no-referrer"),
    ] {
        headers.insert(name, header::HeaderValue::from_static(value));
    }
}

/// `200 text/event-stream` carrying `events` until they end, the client
/// leaves or the portal shuts down.
pub(crate) fn event_stream(
    events: impl Stream<Item = Frame<Bytes>> + Send + 'static,
    shutdown: &Shutdown,
) -> Response {
    let shutdown = shutdown.clone();
    let body = events
        .take_until(async move { shutdown.requested().await })
        .map(Ok::<_, Infallible>);
    let mut response = Response::new(Body::new(StreamBody::new(body)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream"),
    );
    response
}

pub(crate) fn json(status: StatusCode, value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => bytes(status, "application/json", body),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            Failure::Internal {
                message: "response encoding failed".into(),
            },
        ),
    }
}

pub(crate) fn empty(status: StatusCode) -> Response {
    let mut response = Response::new(Full::new(Bytes::new()).boxed_unsync());
    *response.status_mut() = status;
    response
}

pub(crate) fn failure(status: StatusCode, body: Failure) -> Response {
    match serde_json::to_vec(&body) {
        Ok(encoded) => bytes(status, "application/json", encoded),
        Err(_) => empty(status),
    }
}

/// A runtime command failure: the kind, a message (for an adapter failure its
/// whole cause chain, see [`Failure`]) and, where relevant, the field, the
/// current revision or the section.
pub(crate) fn error(error: &Error) -> Response {
    let status = match error {
        Error::Invalid(_) | Error::Decode { .. } | Error::UnusableSettings(_) => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        Error::StaleRevision { .. } | Error::NeedsScreenSync => StatusCode::CONFLICT,
        Error::Faulted(_) | Error::Closed => StatusCode::SERVICE_UNAVAILABLE,
        Error::Adapter { .. } => StatusCode::BAD_GATEWAY,
        Error::Panicked(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    json(status, &Failure::from(error))
}
