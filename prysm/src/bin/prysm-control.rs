use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    process::Command,
    sync::Mutex,
    time::{Instant, sleep, timeout},
};

const UNIT: &str = "prysm-led.service";
const PAGE: &str = include_str!("../control.html");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: SocketAddr,
    #[serde(deserialize_with = "deserialize_url")]
    wled_url: reqwest::Url,
    allowed_hosts: Vec<String>,
    token_file: PathBuf,
    state_dir: PathBuf,
}

fn deserialize_url<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<reqwest::Url, D::Error> {
    let value = String::deserialize(deserializer)?;
    reqwest::Url::parse(&value).map_err(serde::de::Error::custom)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Source {
    Prysm,
    Wled,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Target {
    power: bool,
    source: Source,
}

#[derive(Clone, Debug, Serialize)]
struct Observation {
    power: bool,
    realtime: bool,
    realtime_source: Option<String>,
    live_override: u8,
    service: String,
    substate: String,
    service_result: String,
}
impl Observation {
    fn matches(&self, target: Target) -> bool {
        if !target.power {
            !self.power && !self.realtime && self.live_override == 2 && self.service == "inactive"
        } else if target.source == Source::Prysm {
            self.power && self.realtime && self.live_override == 0 && self.service == "active"
        } else {
            self.power && !self.realtime && self.live_override == 2 && self.service == "inactive"
        }
    }
}

trait Backend {
    async fn service(&self, action: &str) -> Result<()>;
    async fn wled(&self, value: Value) -> Result<()>;
    async fn observe(&self) -> Result<Observation>;
}

struct System {
    client: reqwest::Client,
    url: reqwest::Url,
}
impl System {
    async fn systemctl(args: &[&str]) -> Result<String> {
        let output = timeout(
            Duration::from_secs(15),
            Command::new("systemctl")
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("systemctl timed out")??;
        if !output.status.success() {
            bail!(
                "systemctl failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8(output.stdout)?)
    }
}
impl Backend for System {
    async fn service(&self, action: &str) -> Result<()> {
        Self::systemctl(&["--no-ask-password", action, UNIT]).await?;
        Ok(())
    }
    async fn wled(&self, value: Value) -> Result<()> {
        let response: Value = self
            .client
            .post(self.url.join("json/state")?)
            .json(&value)
            .send()
            .await
            .context("cannot reach WLED")?
            .error_for_status()?
            .json()
            .await?;
        if response.get("error").is_some() {
            bail!("WLED rejected update");
        }
        Ok(())
    }
    async fn observe(&self) -> Result<Observation> {
        let (service, wled) = tokio::join!(
            Self::systemctl(&["show", UNIT, "--property=ActiveState,SubState,Result"]),
            self.client.get(self.url.join("json")?).send()
        );
        let service = service?;
        let wled: Value = wled
            .context("cannot reach WLED")?
            .error_for_status()?
            .json()
            .await?;
        let property = |key: &str| {
            service
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .unwrap_or("unknown")
                .to_owned()
        };
        Ok(Observation {
            power: wled["state"]["on"]
                .as_bool()
                .context("WLED omitted power state")?,
            realtime: wled["info"]["live"]
                .as_bool()
                .context("WLED omitted realtime state")?,
            realtime_source: wled["info"]["lm"].as_str().map(str::to_owned),
            live_override: wled["state"]["lor"]
                .as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .context("WLED omitted live override")?,
            service: property("ActiveState="),
            substate: property("SubState="),
            service_result: property("Result="),
        })
    }
}

async fn apply(backend: &impl Backend, target: Target) -> Result<()> {
    if !target.power {
        // Stop even if WLED is offline; reapply after the last DDP packet so override takes effect.
        let preliminary = backend
            .wled(json!({"lor":2,"live":false,"on":false,"tt":0}))
            .await;
        let stopped = backend.service("stop").await;
        stopped.context("cannot stop Prysm")?;
        backend.wled(json!({"live":false})).await?;
        backend
            .wled(json!({"lor":2,"on":false,"tt":0}))
            .await
            .with_context(|| {
                format!("cannot switch WLED off (initial attempt: {preliminary:?})")
            })?;
    } else if target.source == Source::Wled {
        backend.service("stop").await.context("cannot stop Prysm")?;
        backend.wled(json!({"live":false})).await?;
        backend.wled(json!({"lor":2,"on":true})).await?;
    } else {
        // A previous main-segment realtime session can ignore lor until live has been released.
        backend.wled(json!({"live":false})).await?;
        backend.wled(json!({"lor":0,"on":true})).await?;
        backend
            .service("start")
            .await
            .context("cannot start Prysm")?;
    }
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if backend.observe().await?.matches(target) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("requested state was not confirmed; check capture and WLED");
        }
        sleep(Duration::from_millis(300)).await;
    }
}

async fn switch(backend: &impl Backend, target: Target) -> Result<()> {
    let previous = backend.observe().await;
    // A source choice while observed Off only changes what the next On request starts.
    if !target.power && previous.as_ref().is_ok_and(|state| state.matches(target)) {
        return Ok(());
    }
    if let Err(error) = apply(backend, target).await {
        if target.power && target.source == Source::Prysm {
            backend
                .service("stop")
                .await
                .with_context(|| format!("{error:#}; stopping automatic retries also failed"))?;
            let rollback = previous.map_or(
                Target {
                    power: false,
                    source: Source::Wled,
                },
                |state| Target {
                    power: state.power,
                    source: if state.service == "active"
                        && state.realtime
                        && state.live_override == 0
                    {
                        Source::Prysm
                    } else {
                        Source::Wled
                    },
                },
            );
            if let Err(restore_error) = apply(backend, rollback).await {
                let stopped = backend.service("stop").await;
                bail!(
                    "{error:#}; restoring previous state failed: {restore_error:#}; cancel automatic retries: {stopped:?}"
                );
            }
        }
        return Err(error);
    }
    Ok(())
}

#[derive(Default)]
struct Operation {
    id: u64,
    pending: bool,
    error: Option<String>,
}
struct App {
    config: Config,
    token: String,
    backend: System,
    selected: Mutex<Source>,
    operation: Mutex<Operation>,
}

async fn guard(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let allowed = app.config.allowed_hosts.iter().any(|h| h == host);
    let origin_ok = headers.get(header::ORIGIN).is_none_or(|origin| {
        origin.to_str().is_ok_and(|origin| {
            origin == format!("http://{host}") || origin == format!("https://{host}")
        })
    });
    if !allowed || !origin_ok {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.uri().path().starts_with("/api/") {
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if bearer != Some(app.token.as_str()) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response.headers_mut().insert("content-security-policy", "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    response
}

async fn state(State(app): State<Arc<App>>) -> Json<Value> {
    let observation = app.backend.observe().await;
    let operation = app.operation.lock().await;
    Json(
        json!({"source": *app.selected.lock().await, "observed": observation.as_ref().ok(),
        "observation_error": observation.err().map(|e| format!("{e:#}")), "pending": operation.pending,
        "operation_id": operation.id, "error": operation.error, "wled_url": app.config.wled_url.as_str()}),
    )
}

async fn update(State(app): State<Arc<App>>, Json(target): Json<Target>) -> Response {
    let mut operation = app.operation.lock().await;
    if operation.pending {
        return (
            StatusCode::CONFLICT,
            "A change is already running; retry after it finishes",
        )
            .into_response();
    }
    operation.pending = true;
    operation.error = None;
    operation.id += 1;
    let id = operation.id;
    drop(operation);
    tokio::spawn(async move {
        let result = async {
            switch(&app.backend, target).await?;
            let temporary = app.config.state_dir.join("source.tmp");
            tokio::fs::write(&temporary, serde_json::to_vec(&target.source)?).await?;
            tokio::fs::rename(temporary, app.config.state_dir.join("source.json")).await?;
            *app.selected.lock().await = target.source;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let mut operation = app.operation.lock().await;
        operation.pending = false;
        if let Err(error) = result {
            tracing::error!("control operation {id} failed: {error:#}");
            operation.error = Some(format!("{error:#}"));
        }
    });
    (StatusCode::ACCEPTED, Json(json!({"operation_id": id}))).into_response()
}

#[tokio::main]
async fn main() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("prysm-control requires Linux and systemd");
    }
    tracing_subscriber::fmt::init();
    let path = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("/etc/prysm/control.toml"), PathBuf::from);
    let config: Config = toml::from_str(&tokio::fs::read_to_string(path).await?)?;
    if !matches!(config.wled_url.scheme(), "http" | "https")
        || config.wled_url.host_str().is_none()
        || !config.wled_url.username().is_empty()
        || config.wled_url.password().is_some()
        || config.wled_url.path() != "/"
        || config.wled_url.query().is_some()
        || config.wled_url.fragment().is_some()
        || config.allowed_hosts.is_empty()
    {
        bail!("configure a WLED HTTP base URL and allowed panel hosts");
    }
    let token = tokio::fs::read_to_string(&config.token_file)
        .await?
        .trim()
        .to_owned();
    if token.len() < 32 || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
        bail!("access token must contain at least 32 ASCII letters/digits");
    }
    tokio::fs::create_dir_all(&config.state_dir).await?;
    let selected = match tokio::fs::read(config.state_dir.join("source.json")).await {
        Ok(value) => serde_json::from_slice(&value).context("invalid saved source")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Source::Prysm,
        Err(error) => return Err(error.into()),
    };
    let listen = config.listen;
    let backend = System {
        url: config.wled_url.clone(),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?,
    };
    let app = Arc::new(App {
        config,
        token,
        backend,
        selected: Mutex::new(selected),
        operation: Mutex::new(Operation::default()),
    });
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/api/state", get(state).put(update))
        .layer(DefaultBodyLimit::max(1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("control panel listening on {listen}");
    axum::serve(listener, router).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        state: Mutex<Observation>,
        calls: Mutex<Vec<String>>,
        fail: Option<&'static str>,
    }
    impl Fake {
        fn new(power: bool) -> Self {
            Self {
                state: Mutex::new(Observation {
                    power,
                    realtime: false,
                    realtime_source: None,
                    live_override: 2,
                    service: "inactive".into(),
                    substate: "dead".into(),
                    service_result: "success".into(),
                }),
                calls: Mutex::new(vec![]),
                fail: None,
            }
        }
    }
    impl Backend for Fake {
        async fn service(&self, action: &str) -> Result<()> {
            self.calls.lock().await.push(action.into());
            if self.fail == Some(action)
                || (self.fail == Some("start-stop") && matches!(action, "start" | "stop"))
            {
                bail!("denied");
            }
            let mut state = self.state.lock().await;
            state.service = if action == "start" {
                "active"
            } else {
                "inactive"
            }
            .into();
            if action == "start" {
                state.realtime = true;
            }
            Ok(())
        }
        async fn wled(&self, value: Value) -> Result<()> {
            self.calls.lock().await.push(value.to_string());
            if self.fail == Some("wled") {
                bail!("offline");
            }
            let mut state = self.state.lock().await;
            if let Some(on) = value["on"].as_bool() {
                state.power = on;
            }
            if !state.realtime
                && let Some(lor) = value["lor"].as_u64()
            {
                state.live_override = u8::try_from(lor)?;
            }
            if value["live"] == false {
                state.realtime = false;
            }
            Ok(())
        }
        async fn observe(&self) -> Result<Observation> {
            Ok(self.state.lock().await.clone())
        }
    }
    #[tokio::test]
    async fn all_transitions_and_repeated_requests() {
        let backend = Fake::new(false);
        for target in [
            Target {
                power: false,
                source: Source::Wled,
            },
            Target {
                power: true,
                source: Source::Wled,
            },
            Target {
                power: true,
                source: Source::Prysm,
            },
            Target {
                power: true,
                source: Source::Prysm,
            },
            Target {
                power: false,
                source: Source::Prysm,
            },
        ] {
            switch(&backend, target).await.unwrap();
            assert!(backend.observe().await.unwrap().matches(target));
        }
    }
    #[tokio::test]
    async fn source_selection_while_off_has_no_side_effects() {
        let backend = Fake::new(false);
        switch(
            &backend,
            Target {
                power: false,
                source: Source::Wled,
            },
        )
        .await
        .unwrap();
        assert!(backend.calls.lock().await.is_empty());
    }
    #[tokio::test]
    async fn off_stops_even_when_wled_is_offline() {
        let mut backend = Fake::new(true);
        backend.fail = Some("wled");
        assert!(
            switch(
                &backend,
                Target {
                    power: false,
                    source: Source::Prysm
                }
            )
            .await
            .is_err()
        );
        assert!(backend.calls.lock().await.iter().any(|call| call == "stop"));
    }
    #[tokio::test]
    async fn start_failure_restores_effect_and_cancels_retries() {
        let mut backend = Fake::new(true);
        backend.fail = Some("start");
        assert!(
            switch(
                &backend,
                Target {
                    power: true,
                    source: Source::Prysm
                }
            )
            .await
            .is_err()
        );
        assert!(backend.observe().await.unwrap().matches(Target {
            power: true,
            source: Source::Wled
        }));
        assert!(backend.calls.lock().await.iter().any(|call| call == "stop"));
    }
    #[tokio::test]
    async fn effects_are_never_enabled_when_stop_is_denied() {
        let mut backend = Fake::new(true);
        backend.fail = Some("stop");
        assert!(
            switch(
                &backend,
                Target {
                    power: true,
                    source: Source::Wled
                }
            )
            .await
            .is_err()
        );
        assert_eq!(*backend.calls.lock().await, ["stop"]);
    }
    #[test]
    fn rejects_unknown_and_invalid_fields() {
        for input in [
            r#"{"power":true,"source":"other"}"#,
            r#"{"power":true,"source":"prysm","command":"stop"}"#,
            r#"{"power":"yes","source":"wled"}"#,
        ] {
            assert!(serde_json::from_str::<Target>(input).is_err());
        }
    }
    #[tokio::test]
    async fn compensation_failure_is_reported() {
        let mut backend = Fake::new(true);
        backend.fail = Some("start-stop");
        let error = switch(
            &backend,
            Target {
                power: true,
                source: Source::Prysm,
            },
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("stopping automatic retries also failed"));
    }
    #[tokio::test]
    async fn effects_release_main_segment_before_setting_override() {
        let backend = Fake::new(true);
        {
            let mut state = backend.state.lock().await;
            state.realtime = true;
            state.live_override = 0;
            state.service = "active".into();
        }
        switch(
            &backend,
            Target {
                power: true,
                source: Source::Wled,
            },
        )
        .await
        .unwrap();
        assert!(backend.observe().await.unwrap().matches(Target {
            power: true,
            source: Source::Wled
        }));
        assert_eq!(backend.calls.lock().await[0], "stop");
        assert_eq!(backend.calls.lock().await[1], r#"{"live":false}"#);
    }
    #[tokio::test]
    async fn http_boundary_rejects_untrusted_requests_and_conflicts() {
        use axum::body::Body;
        use tower::ServiceExt;
        let address = "127.0.0.1:8787".parse().unwrap();
        let token = "a".repeat(32);
        let app = Arc::new(App {
            config: Config {
                listen: address,
                wled_url: "http://127.0.0.1/".parse().unwrap(),
                allowed_hosts: vec![address.to_string()],
                token_file: PathBuf::new(),
                state_dir: PathBuf::new(),
            },
            token: token.clone(),
            backend: System {
                client: reqwest::Client::new(),
                url: "http://127.0.0.1/".parse().unwrap(),
            },
            selected: Mutex::new(Source::Prysm),
            operation: Mutex::new(Operation {
                pending: true,
                ..Operation::default()
            }),
        });
        let router = Router::new()
            .route("/", get(|| async { Html(PAGE) }))
            .route("/api/state", get(state).put(update))
            .layer(DefaultBodyLimit::max(1024))
            .layer(middleware::from_fn_with_state(app.clone(), guard))
            .with_state(app);
        let request = |method: &str,
                       host: &str,
                       origin: Option<&str>,
                       bearer: bool,
                       content_type: Option<&str>,
                       body: String| {
            let mut request = Request::builder()
                .method(method)
                .uri("/api/state")
                .header("Host", host);
            if let Some(origin) = origin {
                request = request.header("Origin", origin);
            }
            if bearer {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
            if let Some(content_type) = content_type {
                request = request.header("Content-Type", content_type);
            }
            request.body(Body::from(body)).unwrap()
        };
        let host = address.to_string();
        for (request, status) in [
            (
                request("GET", &host, None, false, None, String::new()),
                StatusCode::UNAUTHORIZED,
            ),
            (
                request("GET", "attacker.example", None, true, None, String::new()),
                StatusCode::FORBIDDEN,
            ),
            (
                request(
                    "PUT",
                    &host,
                    Some("http://attacker.example"),
                    true,
                    Some("application/json"),
                    r#"{"power":false,"source":"prysm"}"#.into(),
                ),
                StatusCode::FORBIDDEN,
            ),
            (
                request(
                    "PUT",
                    &host,
                    None,
                    true,
                    Some("application/json"),
                    r#"{"power":false,"source":"prysm","unknown":1}"#.into(),
                ),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                request(
                    "PUT",
                    &host,
                    None,
                    true,
                    Some("application/json"),
                    "{}".repeat(1024),
                ),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
            (
                request("PUT", &host, None, true, None, "{}".into()),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                request(
                    "PUT",
                    &host,
                    None,
                    true,
                    Some("application/json"),
                    "{".into(),
                ),
                StatusCode::BAD_REQUEST,
            ),
            (
                request(
                    "PUT",
                    &host,
                    Some("http://127.0.0.1:8787"),
                    true,
                    Some("application/json"),
                    r#"{"power":false,"source":"prysm"}"#.into(),
                ),
                StatusCode::CONFLICT,
            ),
        ] {
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                status
            );
        }
    }
}
