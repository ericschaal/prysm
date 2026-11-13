//! The capture stream path: a capture-only host's (the TV's) WebSocket is
//! upgraded here and handed, as a plain blocking socket, to the remote
//! source, which speaks the WebSocket itself on its own thread.
use crate::response::{self, Response, failure};
use crate::routes::Context;
use crate::timed_io::TimedIo;
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use prysm_ports::{Accepted, CAPTURE_PROTOCOL, Handoff};
use prysm_protocol::FailureKind;

/// Answer a WebSocket upgrade with `101` and this protocol version's
/// subprotocol, and hand the connection over once hyper releases it. `426`,
/// naming the version spoken here, when the sender offers another; `503`
/// while no source receives (lighting off, or the capture is not `remote`).
pub(crate) fn upgrade(
    context: &Context,
    handoff: &Handoff,
    mut request: Request<Incoming>,
) -> Response {
    let Some(key) = websocket_key(request.headers()) else {
        return failure(
            StatusCode::BAD_REQUEST,
            FailureKind::BadRequest,
            "expected a WebSocket upgrade",
        );
    };
    if !offers_protocol(request.headers()) {
        let mut refused = failure(
            StatusCode::UPGRADE_REQUIRED,
            FailureKind::BadRequest,
            "the sender speaks another capture stream version; update both",
        );
        refused.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(CAPTURE_PROTOCOL),
        );
        return refused;
    }
    if !handoff.is_receiving() {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            FailureKind::Unavailable,
            "no source receives the capture stream: lighting is off or the capture is not `remote`",
        );
    }
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let upgraded = hyper::upgrade::on(&mut request);
    let handoff = handoff.clone();
    context.spawn_upgrade(async move {
        let upgraded = match upgraded.await {
            Ok(upgraded) => upgraded,
            Err(error) => {
                tracing::debug!(%error, "the capture stream upgrade did not complete");
                return;
            }
        };
        // Always the portal's own connection type.
        let Ok(parts) = upgraded.downcast::<TokioIo<TimedIo>>() else {
            tracing::warn!("the capture stream upgrade is not a portal connection");
            return;
        };
        let socket = match parts.io.into_inner().into_socket().into_std() {
            Ok(socket) => socket,
            Err(error) => {
                tracing::debug!(%error, "the capture stream socket cannot be handed over");
                return;
            }
        };
        if let Err(error) = socket.set_nonblocking(false) {
            tracing::debug!(%error, "the capture stream socket cannot be made blocking");
            return;
        }
        // A source that stopped in between: the connection closes.
        if handoff
            .offer(Accepted {
                socket,
                buffered: parts.read_buf.to_vec(),
            })
            .is_err()
        {
            tracing::debug!("the capture stream arrived after its source stopped");
        }
    });
    let mut response = response::empty(StatusCode::SWITCHING_PROTOCOLS);
    let headers = response.headers_mut();
    headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(CAPTURE_PROTOCOL),
    );
    if let Ok(accept) = HeaderValue::from_str(&accept) {
        headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
    }
    response
}

/// Whether the comma-separated header `name` lists `token` (any case).
fn lists(headers: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
    })
}

/// The sender offers this end's capture stream version.
fn offers_protocol(headers: &HeaderMap) -> bool {
    lists(headers, header::SEC_WEBSOCKET_PROTOCOL, CAPTURE_PROTOCOL)
}

/// The client's key of a version-13 WebSocket upgrade request.
fn websocket_key(headers: &HeaderMap) -> Option<&str> {
    if !lists(headers, header::UPGRADE, "websocket")
        || !lists(headers, header::CONNECTION, "upgrade")
    {
        return None;
    }
    if headers.get(header::SEC_WEBSOCKET_VERSION)? != "13" {
        return None;
    }
    headers.get(header::SEC_WEBSOCKET_KEY)?.to_str().ok()
}
