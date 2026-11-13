//! Response encoding and the stable mapping from runtime errors to HTTP.
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::{StatusCode, header};
use prysm_ports::Error;
use prysm_protocol::{Failure, FailureKind};
use serde::Serialize;
use std::convert::Infallible;

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

pub(crate) fn secure(response: &mut Response) {
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
        (header::REFERRER_POLICY, "no-referrer"),
    ] {
        headers.insert(name, header::HeaderValue::from_static(value));
    }
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
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    json(status, &body)
}
