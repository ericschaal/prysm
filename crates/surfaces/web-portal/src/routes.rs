//! Routing, request decoding and the state stream.
use crate::FrontendAsset;
use crate::access::{self, Access};
use crate::capture;
use crate::colors::Colors;
use crate::metrics;
use crate::page;
use crate::preview::Previews;
use crate::response::{self, KEEPALIVE, KEEPALIVE_COMMENT, Response, failure};
use crate::sampled::Sampled;
use bytes::Bytes;
use futures::stream;
use http_body_util::{BodyExt, Limited};
use hyper::header::HeaderValue;
use hyper::{Method, Request, StatusCode, body::Frame, body::Incoming, header};
use prysm_ports::{AdapterSettings, CAPTURE_STREAM_PATH, Handoff, Processing, Section};
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
    previews: Sampled<Previews>,
    colors: Sampled<Colors>,
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
            previews: Sampled::new(
                4,
                "too many preview viewers",
                Duration::from_millis(125),
                Previews::default(),
            ),
            colors: Sampled::new(
                8,
                "too many colors streams",
                Duration::from_millis(100),
                Colors,
            ),
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
    let path = request.uri().path();
    // An enabled `/metrics` follows the API's origin policy.
    if !path.starts_with("/api/") && !(context.metrics && path == METRICS) {
        return asset(context, path, &request);
    }
    let mut response = match access::check(&context.access, request.headers()) {
        Err(denied) => failure(denied),
        Ok(Some(origin)) if request.method() == Method::OPTIONS => {
            access::preflight(origin, request.headers())
        }
        Ok(cors) => {
            let mut response = api(context, request).await.unwrap_or_else(failure);
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
        return failure(not_found());
    }
    let path = if path == "/" { page::PAGE } else { path };
    let Some(asset) = context.assets.iter().find(|asset| asset.path == path) else {
        return failure(not_found());
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

/// Match each known path and method together. Unknown paths return 404 and
/// unsupported methods on a known path return 405 before the body is read.
/// The engine bounds its commands. Disconnecting after dispatch leaves the
/// command running, and its outcome is visible on the state stream.
async fn api(context: &Context, request: Request<Incoming>) -> Result<Response, Failure> {
    let path = request.uri().path();
    let require_method = |expected: Method| {
        if request.method() == expected {
            Ok(())
        } else {
            Err(method_not_allowed())
        }
    };
    Ok(match path {
        "/api/v1/state" => {
            require_method(Method::GET)?;
            response::json(
                StatusCode::OK,
                &State::from(&*context.engine.snapshot().borrow()),
            )
        }
        "/api/v1/state/stream" => {
            require_method(Method::GET)?;
            stream_state(context)
        }
        METRICS => {
            require_method(Method::GET)?;
            let text =
                metrics::render(&context.engine.stats(), &context.engine.snapshot().borrow());
            response::bytes(StatusCode::OK, metrics::CONTENT_TYPE, text)
        }
        "/api/v1/preview/stream" => {
            require_method(Method::GET)?;
            context
                .previews
                .stream(|| context.engine.preview(), &context.shutdown)
        }
        "/api/v1/colors/stream" => {
            require_method(Method::GET)?;
            context
                .colors
                .stream(|| context.engine.colors(), &context.shutdown)
        }
        CAPTURE_STREAM_PATH => {
            let handoff = context.capture.as_ref().ok_or_else(not_found)?;
            require_method(Method::GET)?;
            capture::upgrade(context, handoff, request)
        }
        "/api/v1/target" => {
            require_method(Method::PUT)?;
            let body: TargetRequest = decode(request, Body::Request).await?;
            context.engine.set_target(body.target).await?;
            response::empty(StatusCode::NO_CONTENT)
        }
        "/api/v1/calibration" => {
            let pattern = match *request.method() {
                Method::PUT => Some(decode(request, Body::Request).await?),
                Method::DELETE => None,
                _ => return Err(method_not_allowed()),
            };
            context.engine.set_calibration(pattern).await?;
            response::empty(StatusCode::NO_CONTENT)
        }
        "/api/v1/processing/trial" => {
            require_method(Method::PUT)?;
            let body: Processing = decode(request, Body::Section(Section::Processing)).await?;
            context.engine.try_processing(body).await?;
            response::empty(StatusCode::NO_CONTENT)
        }
        "/api/v1/settings" => {
            require_method(Method::PUT)?;
            let body: SettingsRequest = decode(request, Body::Settings).await?;
            let revision = context
                .engine
                .replace_settings(body.expected, body.settings)
                .await?;
            response::json(StatusCode::OK, &SettingsReplaced { revision })
        }
        "/api/v1/capture/modes" => {
            require_method(Method::POST)?;
            let capture: AdapterSettings = decode(request, Body::Section(Section::Capture)).await?;
            let modes = context.engine.capture_modes(capture).await?;
            response::json(StatusCode::OK, &modes)
        }
        _ => {
            let rest = path.strip_prefix(CANDIDATES).ok_or_else(not_found)?;
            let (section, kind) = rest.split_once('/').ok_or_else(not_found)?;
            let section = match section {
                "capture" => Section::Capture,
                "output" => Section::Output,
                _ => return Err(not_found()),
            };
            if kind.is_empty() || kind.contains('/') {
                return Err(not_found());
            }
            require_method(Method::GET)?;
            let found = context.engine.candidates(section, kind).await?;
            response::json(StatusCode::OK, &found)
        }
    })
}

fn not_found() -> Failure {
    Failure::NotFound {
        message: "not found".into(),
    }
}

fn method_not_allowed() -> Failure {
    Failure::MethodNotAllowed {
        message: "method not allowed".into(),
    }
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
) -> Result<T, Failure> {
    let json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Err(Failure::UnsupportedMediaType {
            message: "application/json required".into(),
        });
    }
    let body = Limited::new(request.into_body(), BODY_LIMIT).collect();
    let body = match timeout(BODY_DEADLINE, body).await {
        Err(_) => {
            return Err(Failure::RequestTimeout {
                message: "request body timed out".into(),
            });
        }
        Ok(Err(error)) if error.is::<http_body_util::LengthLimitError>() => {
            return Err(Failure::PayloadTooLarge {
                message: "request body exceeds 1 MiB".into(),
            });
        }
        Ok(Err(_)) => {
            return Err(Failure::BadRequest {
                message: "request body unreadable".into(),
            });
        }
        Ok(Ok(body)) => body.to_bytes(),
    };
    let malformed = |error: &serde_json::Error| Failure::BadRequest {
        message: format!("invalid request: {error}"),
    };
    let mut json = serde_json::Deserializer::from_slice(&body);
    let value = serde_path_to_error::deserialize(&mut json).map_err(|error| {
        let field = (error.inner().classify() == serde_json::error::Category::Data)
            .then(|| settings.field(error.path()))
            .flatten();
        match field {
            Some(field) => Failure::Invalid {
                message: error.inner().to_string(),
                field: Some(field),
            },
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
        return failure(Failure::Busy {
            message: "too many state streams".into(),
        });
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
            &["gzip;q=0, *"],
            &["*", "gzip;q=0"],
        ] {
            assert!(!accepts(refused), "{refused:?}");
        }
    }
}
