//! Response encoding and the stable mapping from runtime errors to HTTP.
use bytes::Bytes;
use futures::{Stream, StreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{StatusCode, body::Frame, header};
use prysm_ports::Error;
use prysm_protocol::{Failure, FailureKind};
use prysm_runtime::Shutdown;
use serde::Serialize;
use std::convert::Infallible;
use std::time::Duration;

/// An event stream with nothing new sends [`KEEPALIVE_COMMENT`] this often,
/// so a vanished reader is noticed by the write deadline and releases its
/// stream slot.
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
            FailureKind::Internal,
            "response encoding failed",
        ),
    }
}

pub(crate) fn empty(status: StatusCode) -> Response {
    let mut response = Response::new(Full::new(Bytes::new()).boxed_unsync());
    *response.status_mut() = status;
    response
}

pub(crate) fn failure(status: StatusCode, kind: FailureKind, message: &str) -> Response {
    let body = Failure::new(kind, message);
    match serde_json::to_vec(&body) {
        Ok(encoded) => bytes(status, "application/json", encoded),
        Err(_) => empty(status),
    }
}

/// A runtime command failure. Source chains stay local; the body carries the
/// kind, a message and, where relevant, the field or current revision.
pub(crate) fn error(error: &Error) -> Response {
    let body = Failure::from(error);
    let status = match body.kind {
        FailureKind::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        FailureKind::StaleRevision | FailureKind::NotProcessed => StatusCode::CONFLICT,
        FailureKind::Faulted | FailureKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        FailureKind::Adapter => StatusCode::BAD_GATEWAY,
        // Not what a runtime error becomes; listed so a new kind is mapped here.
        FailureKind::Timeout
        | FailureKind::Internal
        | FailureKind::Forbidden
        | FailureKind::NotFound
        | FailureKind::MethodNotAllowed
        | FailureKind::BadRequest
        | FailureKind::PayloadTooLarge
        | FailureKind::UnsupportedMediaType
        | FailureKind::Busy => StatusCode::INTERNAL_SERVER_ERROR,
    };
    json(status, &body)
}
