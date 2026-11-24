//! Routing, request decoding and the state stream.
use crate::FrontendAsset;
use crate::access::{self, Access};
use crate::capture;
use crate::colors::Colors;
use crate::metrics;
use crate::page;
use crate::preview::Previews;
use crate::response::{self, KEEPALIVE, KEEPALIVE_COMMENT, Response, failure};
use bytes::Bytes;
use futures::stream;
use http_body_util::{BodyExt, Limited};
use hyper::header::HeaderValue;
use hyper::{Method, Request, StatusCode, body::Frame, body::Incoming, header};
use prysm_ports::{
    AdapterSettings, CAPTURE_STREAM_PATH, CalibrationPattern, Handoff, Processing, Result, Section,
};
use prysm_protocol::{Failure, SettingsReplaced, SettingsRequest, State, TargetRequest};
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
/// it, cached as the asset says. The page carries the current state, so it
/// is never stored.
fn asset(context: &Context, path: &str, request: &Request<Incoming>) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return not_found();
    }
    let path = if path == "/" { page::PAGE } else { path };
    let Some(asset) = context.assets.iter().find(|asset| asset.path == path) else {
        return not_found();
    };
    if asset.path == page::PAGE {
        let state = State::from(&*context.engine.snapshot().borrow());
        let body = page::with_state(asset.bytes, &state);
        return response::bytes(StatusCode::OK, asset.content_type, body);
    }
    let gzip = asset.gzip.filter(|_| accepts_gzip(request.headers()));
    let mut response = response::bytes(
        StatusCode::OK,
        asset.content_type,
        gzip.unwrap_or(asset.bytes),
    );
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(asset.caching.header()),
    );
    // Either answer depends on `Accept-Encoding`, so caches keep both apart.
    if asset.gzip.is_some() {
        headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    if gzip.is_some() {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    response
}

/// Whether `Accept-Encoding` accepts gzip: it names gzip, or else `*`,
/// without `q=0` (RFC 9110, 12.5.3). Without the header, or with neither,
/// the file is sent as it is.
fn accepts_gzip(headers: &header::HeaderMap) -> bool {
    let (mut gzip, mut any) = (None, None);
    let codings = headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','));
    for coding in codings {
        let mut parts = coding.split(';').map(str::trim);
        let name = parts.next().unwrap_or_default();
        let accepted = !parts.any(|parameter| {
            parameter.split_once('=').is_some_and(|(key, q)| {
                // Zero is "0", "0.0" and so on: exactly 0 as a float.
                key.trim().eq_ignore_ascii_case("q") && q.trim().parse::<f32>() == Ok(0.0)
            })
        });
        if name.eq_ignore_ascii_case("gzip") {
            gzip = Some(accepted);
        } else if name == "*" {
            any = Some(accepted);
        }
    }
    gzip.or(any).unwrap_or(false)
}

/// An API path, each written once; a path that is none of these is `404`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route<'a> {
    State,
    StateStream,
    Metrics,
    PreviewStream,
    ColorsStream,
    CaptureStream,
    Target,
    Calibration,
    ProcessingTrial,
    Settings,
    CaptureModes,
    /// What follows [`CANDIDATES`]: `{section}/{kind}`.
    Candidates(&'a str),
}

impl<'a> Route<'a> {
    fn of(path: &'a str) -> Option<Self> {
        Some(match path {
            "/api/v1/state" => Self::State,
            "/api/v1/state/stream" => Self::StateStream,
            METRICS => Self::Metrics,
            "/api/v1/preview/stream" => Self::PreviewStream,
            "/api/v1/colors/stream" => Self::ColorsStream,
            CAPTURE_STREAM_PATH => Self::CaptureStream,
            "/api/v1/target" => Self::Target,
            "/api/v1/calibration" => Self::Calibration,
            "/api/v1/processing/trial" => Self::ProcessingTrial,
            "/api/v1/settings" => Self::Settings,
            "/api/v1/capture/modes" => Self::CaptureModes,
            _ => return path.strip_prefix(CANDIDATES).map(Self::Candidates),
        })
    }
}

/// One API path's methods. A known path with another method is `405`, an
/// unknown path `404`.
async fn api(context: &Context, path: &str, request: Request<Incoming>) -> Response {
    let Some(route) = Route::of(path) else {
        return not_found();
    };
    let method = request.method().clone();
    match (route, method) {
        (Route::State, Method::GET) => response::json(
            StatusCode::OK,
            &State::from(&*context.engine.snapshot().borrow()),
        ),
        (Route::StateStream, Method::GET) => stream_state(context),
        (Route::Metrics, Method::GET) => {
            let text =
                metrics::render(&context.engine.stats(), &context.engine.snapshot().borrow());
            response::bytes(StatusCode::OK, metrics::CONTENT_TYPE, text)
        }
        (Route::PreviewStream, Method::GET) => {
            context.previews.stream(&context.engine, &context.shutdown)
        }
        (Route::ColorsStream, Method::GET) => {
            context.colors.stream(&context.engine, &context.shutdown)
        }
        (Route::CaptureStream, method) => match &context.capture {
            Some(handoff) if method == Method::GET => capture::upgrade(context, handoff, request),
            Some(_) => method_not_allowed(),
            None => not_found(),
        },
        (Route::Target, Method::PUT) => {
            match decode::<TargetRequest>(request, Body::Request).await {
                Ok(body) => applied(command(context.engine.set_target(body.target)).await),
                Err(response) => *response,
            }
        }
        (Route::Calibration, Method::PUT) => {
            match decode::<CalibrationPattern>(request, Body::Request).await {
                Ok(body) => applied(command(context.engine.set_calibration(Some(body))).await),
                Err(response) => *response,
            }
        }
        (Route::Calibration, Method::DELETE) => {
            applied(command(context.engine.set_calibration(None)).await)
        }
        (Route::ProcessingTrial, Method::PUT) => {
            match decode::<Processing>(request, Body::Section(Section::Processing)).await {
                Ok(body) => applied(command(context.engine.try_processing(body)).await),
                Err(response) => *response,
            }
        }
        (Route::Settings, Method::PUT) => replace_settings(context, request).await,
        (Route::CaptureModes, Method::POST) => {
            match decode::<AdapterSettings>(request, Body::Section(Section::Capture)).await {
                Ok(body) => match command(context.engine.capture_modes(body)).await {
                    Ok(modes) => response::json(StatusCode::OK, &modes),
                    Err(response) => *response,
                },
                Err(response) => *response,
            }
        }
        (Route::Candidates(rest), method) => candidates(context, rest, &method).await,
        _ => method_not_allowed(),
    }
}

/// `PUT /api/v1/settings`: the new revision once saved.
async fn replace_settings(context: &Context, request: Request<Incoming>) -> Response {
    let body = match decode::<SettingsRequest>(request, Body::Settings).await {
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
    failure(
        StatusCode::NOT_FOUND,
        Failure::NotFound {
            message: "not found".into(),
        },
    )
}

fn method_not_allowed() -> Response {
    failure(
        StatusCode::METHOD_NOT_ALLOWED,
        Failure::MethodNotAllowed {
            message: "method not allowed".into(),
        },
    )
}

/// What the adapter for `capture/<kind>` or `output/<kind>` finds to choose
/// from. The adapter bounds the lookup; the engine runs it once at a time.
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

/// Await an engine command: every one ends, since the engine bounds a
/// session's stop and each adapter its own calls, so the portal sets no
/// deadline of its own. A dropped request does not cancel a command the
/// engine already received; the state stream shows its outcome.
async fn command<T>(call: impl Future<Output = Result<T>>) -> Result<T, Box<Response>> {
    call.await
        .map_err(|error| Box::new(response::error(&error)))
}

/// Where a request body holds settings. A value there that does not decode
/// is `422 invalid` with its `field`; any other problem (a body that is not
/// JSON, a field around the settings unknown, missing or of the wrong type)
/// is `400 bad_request`.
#[derive(Debug, Clone, Copy)]
enum Body {
    /// None: a request such as `TargetRequest`.
    Request,
    /// In `SettingsRequest.settings`, whose paths are field paths as they
    /// are (`processing.style.kind`).
    Settings,
    /// Inside the body, which is one section (`Processing` is
    /// `processing`); the body as a whole is the request.
    Section(Section),
}

impl Body {
    /// The settings field at `path` in the body, if it is one; never the
    /// body as a whole.
    fn field(self, path: &serde_path_to_error::Path) -> Option<String> {
        path.iter().next()?;
        let path = path.to_string();
        match self {
            Self::Request => None,
            Self::Settings => path.strip_prefix("settings.").map(str::to_owned),
            Self::Section(section) => Some(format!("{section}.{path}")),
        }
    }
}

async fn decode<T: DeserializeOwned>(
    request: Request<Incoming>,
    settings: Body,
) -> Result<T, Box<Response>> {
    let reject = |status, body| Box::new(failure(status, body));
    let json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Err(reject(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Failure::UnsupportedMediaType {
                message: "application/json required".into(),
            },
        ));
    }
    let body = Limited::new(request.into_body(), BODY_LIMIT).collect();
    let body = match timeout(BODY_DEADLINE, body).await {
        Err(_) => {
            return Err(reject(
                StatusCode::REQUEST_TIMEOUT,
                Failure::RequestTimeout {
                    message: "request body timed out".into(),
                },
            ));
        }
        Ok(Err(error)) if error.is::<http_body_util::LengthLimitError>() => {
            return Err(reject(
                StatusCode::PAYLOAD_TOO_LARGE,
                Failure::PayloadTooLarge {
                    message: "request body exceeds 1 MiB".into(),
                },
            ));
        }
        Ok(Err(_)) => {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                Failure::BadRequest {
                    message: "request body unreadable".into(),
                },
            ));
        }
        Ok(Ok(body)) => body.to_bytes(),
    };
    let malformed = |error: &serde_json::Error| {
        reject(
            StatusCode::BAD_REQUEST,
            Failure::BadRequest {
                message: format!("invalid request: {error}"),
            },
        )
    };
    let mut json = serde_json::Deserializer::from_slice(&body);
    let value = serde_path_to_error::deserialize(&mut json).map_err(|error| {
        let field = (error.inner().classify() == serde_json::error::Category::Data)
            .then(|| settings.field(error.path()))
            .flatten();
        match field {
            Some(field) => reject(
                StatusCode::UNPROCESSABLE_ENTITY,
                Failure::Invalid {
                    message: error.inner().to_string(),
                    field: Some(field),
                },
            ),
            None => malformed(error.inner()),
        }
    })?;
    json.end().map_err(|error| malformed(&error))?;
    Ok(value)
}

/// Server-sent events: the current state at once, then the latest state after
/// every change. Slow readers skip intermediate states. Ends when the engine
/// stops, the portal shuts down or the client goes away.
fn stream_state(context: &Context) -> Response {
    let Ok(permit) = context.streams.clone().try_acquire_owned() else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            Failure::Busy {
                message: "too many state streams".into(),
            },
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

#[cfg(test)]
mod tests {
    use super::accepts_gzip;
    use hyper::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue};

    fn accepts(values: &[&'static str]) -> bool {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(ACCEPT_ENCODING, HeaderValue::from_static(value));
        }
        accepts_gzip(&headers)
    }

    #[test]
    fn gzip_is_sent_when_accept_encoding_names_it_or_any_coding() {
        for accepted in [
            &["gzip"][..],
            &["gzip, deflate, br, zstd"],
            &["GZip;q=0.5"],
            &["gzip; Q=0.001"],
            &["*"],
            &["br;q=0", "*;q=0.1"],
            &["br", "gzip"],
            &["gzip;q=0", "gzip"],
        ] {
            assert!(accepts(accepted), "{accepted:?}");
        }
    }

    #[test]
    fn gzip_is_not_sent_when_refused_or_not_named() {
        for refused in [
            &[][..],
            &[""],
            &["identity"],
            &["br, deflate"],
            &["gzip;q=0"],
            &["gzip; q=0.000"],
            &["*;q=0"],
            // Named gzip outranks `*` either way.
            &["gzip;q=0, *"],
            &["*", "gzip;q=0"],
        ] {
            assert!(!accepts(refused), "{refused:?}");
        }
    }
}
