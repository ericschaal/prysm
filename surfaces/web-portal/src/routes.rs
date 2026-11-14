//! Routing, request decoding and the state stream.
use crate::FrontendAsset;
use crate::access::{self, Access};
use crate::capture;
use crate::colors::Colors;
use crate::metrics;
use crate::preview::Previews;
use crate::response::{self, KEEPALIVE, KEEPALIVE_COMMENT, Response, failure};
use bytes::Bytes;
use futures::stream;
use http_body_util::{BodyExt, Limited};
use hyper::header::HeaderValue;
use hyper::{Method, Request, StatusCode, body::Frame, body::Incoming, header};
use prysm_ports::{CAPTURE_STREAM_PATH, Calibration, Handoff, Processing, Result, Section};
use prysm_protocol::{FailureKind, SettingsReplaced, SettingsRequest, State, TargetRequest};
use prysm_runtime::{EngineHandle, Shutdown};
use serde::de::DeserializeOwned;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, Interval, interval_at, timeout};

const BODY_LIMIT: usize = 1024 * 1024;
pub(crate) const BODY_DEADLINE: Duration = Duration::from_secs(5);
/// Longer than the engine's default stop deadline plus an adapter open.
const COMMAND_DEADLINE: Duration = Duration::from_secs(15);
const STREAMS: usize = 8;
/// The Prometheus scrape path, served only when the host enables it.
const METRICS: &str = "/metrics";
/// `GET /api/v1/candidates/{section}/{kind}`.
const CANDIDATES: &str = "/api/v1/candidates/";

pub(crate) struct Context {
    pub engine: EngineHandle,
    pub assets: &'static [FrontendAsset],
    pub access: Access,
    /// `GET /metrics` is served.
    pub metrics: bool,
    /// Where capture stream connections go; `None`: the path is not served.
    pub capture: Option<Handoff>,
    pub shutdown: Shutdown,
    /// Capture stream upgrades waiting for hyper to release their
    /// connection; joined by the server after the connections.
    upgrades: Mutex<JoinSet<()>>,
    streams: Arc<Semaphore>,
    previews: Previews,
    colors: Colors,
}

impl Context {
    pub(crate) fn new(
        engine: EngineHandle,
        assets: &'static [FrontendAsset],
        access: Access,
        metrics: bool,
        capture: Option<Handoff>,
        shutdown: Shutdown,
    ) -> Self {
        Self {
            engine,
            assets,
            access,
            metrics,
            capture,
            shutdown,
            upgrades: Mutex::default(),
            streams: Arc::new(Semaphore::new(STREAMS)),
            previews: Previews::new(),
            colors: Colors::new(),
        }
    }
}

impl Context {
    /// Run `task` as part of the server: it is joined (or aborted) before
    /// the server returns. Finished ones are collected here.
    pub(crate) fn spawn_upgrade(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut upgrades = self.upgrades.lock().unwrap_or_else(PoisonError::into_inner);
        while upgrades.try_join_next().is_some() {}
        upgrades.spawn(task);
    }

    /// The upgrade tasks still running, for the server to join.
    pub(crate) fn take_upgrades(&self) -> JoinSet<()> {
        std::mem::take(&mut *self.upgrades.lock().unwrap_or_else(PoisonError::into_inner))
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
        return asset(context, &path, &request);
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

/// `GET` or `HEAD` of a bundle file: gzip-compressed for clients that accept
/// it, kept for good when its name carries its hash, else checked every load.
fn asset(context: &Context, path: &str, request: &Request<Incoming>) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return not_found();
    }
    let path = if path == "/" { "/index.html" } else { path };
    let Some(asset) = context.assets.iter().find(|asset| asset.path == path) else {
        return not_found();
    };
    let gzip = asset.gzip.filter(|_| accepts_gzip(request.headers()));
    let mut response = response::bytes(
        StatusCode::OK,
        asset.content_type,
        gzip.unwrap_or(asset.bytes),
    );
    let headers = response.headers_mut();
    let cache = if asset.immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if asset.gzip.is_some() {
        headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    if gzip.is_some() {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    response
}

/// Whether `Accept-Encoding` lists gzip (or `*`) without refusing it (`q=0`).
fn accepts_gzip(headers: &header::HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let name = parts.next().unwrap_or_default();
            let refused = parts.any(|part| {
                part.strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
                    .is_some_and(|q| q == 0.0)
            });
            (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
        })
}

/// One API path's methods. A known path with another method is `405`, an
/// unknown path `404`.
async fn api(context: &Context, path: &str, request: Request<Incoming>) -> Response {
    let method = request.method().clone();
    let get = method == Method::GET;
    match path {
        "/api/v1/state" if get => response::json(
            StatusCode::OK,
            &State::from(&*context.engine.snapshot().borrow()),
        ),
        "/api/v1/state/stream" if get => stream_state(context),
        METRICS if get => {
            let text =
                metrics::render(&context.engine.stats(), &context.engine.snapshot().borrow());
            response::bytes(StatusCode::OK, metrics::CONTENT_TYPE, text)
        }
        "/api/v1/preview/stream" if get => {
            context.previews.stream(&context.engine, &context.shutdown)
        }
        "/api/v1/colors/stream" if get => context.colors.stream(&context.engine, &context.shutdown),
        CAPTURE_STREAM_PATH => match &context.capture {
            Some(handoff) if get => capture::upgrade(context, handoff, request),
            Some(_) => method_not_allowed(),
            None => not_found(),
        },
        "/api/v1/target" if method == Method::PUT => match decode::<TargetRequest>(request).await {
            Ok(body) => applied(command(context.engine.set_target(body.target)).await),
            Err(response) => *response,
        },
        "/api/v1/calibration" if method == Method::PUT => {
            match decode::<Calibration>(request).await {
                Ok(body) => applied(command(context.engine.set_calibration(Some(body))).await),
                Err(response) => *response,
            }
        }
        "/api/v1/calibration" if method == Method::DELETE => {
            applied(command(context.engine.set_calibration(None)).await)
        }
        "/api/v1/processing/trial" if method == Method::PUT => {
            match decode::<Processing>(request).await {
                Ok(body) => applied(command(context.engine.try_processing(body)).await),
                Err(response) => *response,
            }
        }
        "/api/v1/settings" if method == Method::PUT => replace_settings(context, request).await,
        _ if path.starts_with(CANDIDATES) => {
            candidates(context, &path[CANDIDATES.len()..], &method).await
        }
        "/api/v1/state"
        | "/api/v1/state/stream"
        | "/api/v1/preview/stream"
        | "/api/v1/colors/stream"
        | "/api/v1/target"
        | "/api/v1/calibration"
        | "/api/v1/processing/trial"
        | "/api/v1/settings"
        | METRICS => method_not_allowed(),
        _ => not_found(),
    }
}

/// `PUT /api/v1/settings`: the new revision once saved.
async fn replace_settings(context: &Context, request: Request<Incoming>) -> Response {
    let body = match decode::<SettingsRequest>(request).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
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

fn not_found() -> Response {
    failure(StatusCode::NOT_FOUND, FailureKind::NotFound, "not found")
}

fn method_not_allowed() -> Response {
    failure(
        StatusCode::METHOD_NOT_ALLOWED,
        FailureKind::MethodNotAllowed,
        "method not allowed",
    )
}

/// What the adapter for `capture/<kind>` or `output/<kind>` finds to choose
/// from. The adapter bounds the lookup; the command deadline backs it up.
async fn candidates(context: &Context, rest: &str, method: &Method) -> Response {
    let section = match rest.split_once('/') {
        Some(("capture", kind)) => Some((Section::Capture, kind)),
        Some(("output", kind)) => Some((Section::Output, kind)),
        _ => None,
    };
    let Some((section, kind)) = section.filter(|(_, kind)| !kind.is_empty() && !kind.contains('/'))
    else {
        return not_found();
    };
    if method != Method::GET {
        return method_not_allowed();
    }
    match command(context.engine.candidates(section, kind)).await {
        Ok(found) => response::json(StatusCode::OK, &found),
        Err(response) => *response,
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
    response::event_stream(stream::unfold(events, Events::next), &context.shutdown)
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
                Bytes::from(format!("data: {}\n\n", serde_json::to_string(&state).ok()?))
            }
            _ = self.keepalive.tick() => KEEPALIVE_COMMENT,
        };
        Some((Frame::data(event), self))
    }
}
