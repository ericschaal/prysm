//! Routing, request decoding and the state stream.
use crate::FrontendAsset;
use crate::access::{self, Access};
use crate::metrics;
use crate::preview::Previews;
use crate::response::{self, Body, Response, failure};
use bytes::Bytes;
use futures::{StreamExt, stream};
use http_body_util::{BodyExt, Limited, StreamBody};
use hyper::{Method, Request, StatusCode, body::Frame, body::Incoming, header};
use prysm_ports::{Calibration, Result};
use prysm_protocol::{FailureKind, SettingsReplaced, SettingsRequest, State, TargetRequest};
use prysm_runtime::{EngineHandle, Shutdown};
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::time::{Instant, Interval, interval_at, timeout};

const BODY_LIMIT: usize = 1024 * 1024;
pub(crate) const BODY_DEADLINE: Duration = Duration::from_secs(5);
/// Longer than the engine's default stop deadline plus an adapter open.
const COMMAND_DEADLINE: Duration = Duration::from_secs(15);
const STREAMS: usize = 8;
/// Idle streams send a comment this often so a vanished reader is noticed by
/// the write deadline and releases its stream slot.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// The Prometheus scrape path, served only when the host enables it.
const METRICS: &str = "/metrics";

pub(crate) struct Context {
    pub engine: EngineHandle,
    pub assets: &'static [FrontendAsset],
    pub access: Access,
    /// `GET /metrics` is served.
    pub metrics: bool,
    pub shutdown: Shutdown,
    streams: Arc<Semaphore>,
    previews: Previews,
}

impl Context {
    pub(crate) fn new(
        engine: EngineHandle,
        assets: &'static [FrontendAsset],
        access: Access,
        metrics: bool,
        shutdown: Shutdown,
    ) -> Self {
        Self {
            engine,
            assets,
            access,
            metrics,
            shutdown,
            streams: Arc::new(Semaphore::new(STREAMS)),
            previews: Previews::new(),
        }
    }
}

pub(crate) async fn handle(context: Arc<Context>, request: Request<Incoming>) -> Response {
    let mut response = route(&context, request).await;
    response::secure(&mut response);
    response
}

async fn route(context: &Context, request: Request<Incoming>) -> Response {
    let path = request.uri().path().to_owned();
    // An enabled `/metrics` follows the API's origin policy.
    if !path.starts_with("/api/") && !(context.metrics && path == METRICS) {
        return asset(context, &path, request.method());
    }
    let mut response = match access::check(&context.access, request.headers()) {
        Err(denied) => *denied,
        Ok(Some(origin)) if request.method() == Method::OPTIONS => {
            access::preflight(origin, request.headers())
        }
        Ok(cors) => {
            let mut response = api(context, &path, request).await;
            if let Some(origin) = cors {
                access::allow(&mut response, origin);
            }
            response
        }
    };
    response
        .headers_mut()
        .append(header::VARY, header::HeaderValue::from_static("origin"));
    response
}

fn asset(context: &Context, path: &str, method: &Method) -> Response {
    if method != Method::GET {
        return failure(StatusCode::NOT_FOUND, FailureKind::NotFound, "not found");
    }
    let path = if path == "/" { "/index.html" } else { path };
    match context.assets.iter().find(|asset| asset.path == path) {
        Some(asset) => response::bytes(StatusCode::OK, asset.content_type, asset.bytes),
        None => failure(StatusCode::NOT_FOUND, FailureKind::NotFound, "not found"),
    }
}

async fn api(context: &Context, path: &str, request: Request<Incoming>) -> Response {
    let method = request.method().clone();
    match (path, method) {
        ("/api/v1/state", Method::GET) => response::json(
            StatusCode::OK,
            &State::from(&*context.engine.snapshot().borrow()),
        ),
        ("/api/v1/state/stream", Method::GET) => stream_state(context),
        (METRICS, Method::GET) => {
            let text =
                metrics::render(&context.engine.stats(), &context.engine.snapshot().borrow());
            response::bytes(StatusCode::OK, metrics::CONTENT_TYPE, text)
        }
        ("/api/v1/preview/stream", Method::GET) => {
            context.previews.stream(&context.engine, &context.shutdown)
        }
        ("/api/v1/target", Method::PUT) => match decode::<TargetRequest>(request).await {
            Ok(body) => applied(command(context.engine.set_target(body.target)).await),
            Err(response) => *response,
        },
        ("/api/v1/calibration", Method::PUT) => match decode::<Calibration>(request).await {
            Ok(body) => applied(command(context.engine.set_calibration(Some(body))).await),
            Err(response) => *response,
        },
        ("/api/v1/calibration", Method::DELETE) => {
            applied(command(context.engine.set_calibration(None)).await)
        }
        ("/api/v1/settings", Method::PUT) => match decode::<SettingsRequest>(request).await {
            Ok(body) => {
                match command(
                    context
                        .engine
                        .replace_settings(body.expected, body.settings),
                )
                .await
                {
                    Ok(revision) => response::json(StatusCode::OK, &SettingsReplaced { revision }),
                    Err(response) => *response,
                }
            }
            Err(response) => *response,
        },
        (
            "/api/v1/state"
            | "/api/v1/state/stream"
            | "/api/v1/preview/stream"
            | "/api/v1/target"
            | "/api/v1/calibration"
            | "/api/v1/settings"
            | METRICS,
            _,
        ) => failure(
            StatusCode::METHOD_NOT_ALLOWED,
            FailureKind::MethodNotAllowed,
            "method not allowed",
        ),
        _ => failure(StatusCode::NOT_FOUND, FailureKind::NotFound, "not found"),
    }
}

/// `204` once a command without a reply value has been applied.
fn applied(result: Result<(), Box<Response>>) -> Response {
    match result {
        Ok(()) => response::empty(StatusCode::NO_CONTENT),
        Err(response) => *response,
    }
}

/// Await an engine command. A dropped request does not cancel a command the
/// engine already received; a timeout leaves its outcome to the state stream.
async fn command<T>(call: impl Future<Output = Result<T>>) -> Result<T, Box<Response>> {
    match timeout(COMMAND_DEADLINE, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(Box::new(response::error(&error))),
        Err(_) => Err(Box::new(failure(
            StatusCode::GATEWAY_TIMEOUT,
            FailureKind::Timeout,
            "the command did not finish in time; watch the state for its outcome",
        ))),
    }
}

async fn decode<T: DeserializeOwned>(request: Request<Incoming>) -> Result<T, Box<Response>> {
    let reject = |status, kind, message: &str| Box::new(failure(status, kind, message));
    let json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Err(reject(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            FailureKind::UnsupportedMediaType,
            "application/json required",
        ));
    }
    let body = Limited::new(request.into_body(), BODY_LIMIT).collect();
    let body = match timeout(BODY_DEADLINE, body).await {
        Err(_) => {
            return Err(reject(
                StatusCode::REQUEST_TIMEOUT,
                FailureKind::BadRequest,
                "request body timed out",
            ));
        }
        Ok(Err(error)) if error.is::<http_body_util::LengthLimitError>() => {
            return Err(reject(
                StatusCode::PAYLOAD_TOO_LARGE,
                FailureKind::PayloadTooLarge,
                "request body exceeds 1 MiB",
            ));
        }
        Ok(Err(_)) => {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                FailureKind::BadRequest,
                "request body unreadable",
            ));
        }
        Ok(Ok(body)) => body.to_bytes(),
    };
    serde_json::from_slice(&body).map_err(|error| {
        reject(
            StatusCode::BAD_REQUEST,
            FailureKind::BadRequest,
            &format!("invalid request: {error}"),
        )
    })
}

/// Server-sent events: the current state at once, then the latest state after
/// every change. Slow readers skip intermediate states. Ends when the engine
/// stops, the portal shuts down or the client goes away.
fn stream_state(context: &Context) -> Response {
    let Ok(permit) = context.streams.clone().try_acquire_owned() else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            FailureKind::Busy,
            "too many state streams",
        );
    };
    let mut snapshot = context.engine.snapshot();
    snapshot.mark_changed();
    let events = Events {
        snapshot,
        keepalive: interval_at(Instant::now() + KEEPALIVE, KEEPALIVE),
        _permit: permit,
    };
    let shutdown = context.shutdown.clone();
    let body = stream::unfold(events, Events::next)
        .take_until(async move { shutdown.requested().await })
        .map(Ok::<_, Infallible>);
    let mut response = Response::new(Body::new(StreamBody::new(body)));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream"),
    );
    response
}

struct Events {
    snapshot: watch::Receiver<prysm_ports::Snapshot>,
    keepalive: Interval,
    _permit: OwnedSemaphorePermit,
}

impl Events {
    async fn next(mut self) -> Option<(Frame<Bytes>, Self)> {
        let event = tokio::select! {
            changed = self.snapshot.changed() => {
                changed.ok()?;
                let state = State::from(&*self.snapshot.borrow_and_update());
                format!("data: {}\n\n", serde_json::to_string(&state).ok()?)
            }
            _ = self.keepalive.tick() => ":\n\n".to_owned(),
        };
        Some((Frame::data(Bytes::from(event)), self))
    }
}
