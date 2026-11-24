//! The portal under test, its client and the settings the tests start from.
use futures::StreamExt;
use prysm_ports::{AdapterSettings, Settings};
use prysm_runtime::fakes::{
    self, CallLog, FakeOutputSettings, FakeOutputs, FakeSources, MemoryStore,
};
use prysm_runtime::{EngineHandle, Prysm, Shutdown};
pub use prysm_testkit::{eventually_async, within_async};
use prysm_web_portal::{Access, Caching, FrontendAsset, WebPortal};
use reqwest::{Method, RequestBuilder, StatusCode, header};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub const ASSETS: &[FrontendAsset] = &[
    FrontendAsset {
        path: "/index.html",
        content_type: "text/html; charset=utf-8",
        bytes: b"<!doctype html><html><head><title>Prysm</title></head><body></body></html>",
        gzip: None,
        caching: Caching::Never,
    },
    FrontendAsset {
        path: "/assets/app.js",
        content_type: "text/javascript",
        bytes: b"console.log('prysm')",
        // Stands in for the compressed script: the portal sends it as it is.
        gzip: Some(b"compressed"),
        caching: Caching::Immutable,
    },
];
pub const LIMIT: Duration = Duration::from_secs(10);

pub struct Portal {
    pub address: SocketAddr,
    pub client: reqwest::Client,
    pub log: CallLog,
    pub store: MemoryStore,
    pub stop: Option<oneshot::Sender<()>>,
    pub host: JoinHandle<prysm_ports::Result<()>>,
    /// The portal's engine, to observe preview demand.
    pub engine: EngineHandle,
}

pub async fn start(settings: Settings, stop_deadline: Duration) -> Portal {
    start_with(settings, stop_deadline, Access::SameOrigin).await
}

pub async fn start_with(settings: Settings, stop_deadline: Duration, access: Access) -> Portal {
    start_full(settings, stop_deadline, access, false).await
}

pub async fn start_full(
    settings: Settings,
    stop_deadline: Duration,
    access: Access,
    metrics: bool,
) -> Portal {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let portal = WebPortal::from_listener(listener, ASSETS, access)
        .unwrap()
        .metrics(metrics);
    let log = CallLog::new();
    let store = MemoryStore::with(settings);
    let (stop, stopped) = oneshot::channel::<()>();
    let (engine, handed) = oneshot::channel::<EngineHandle>();
    let host = tokio::spawn(
        Prysm::builder(store.clone())
            .source(FakeSources(log.clone()))
            .output(FakeOutputs(log.clone()))
            .surface(portal)
            .surface(move |handle: EngineHandle, shutdown: Shutdown| async move {
                let _ = engine.send(handle);
                shutdown.requested().await;
                Ok(())
            })
            .stop_deadline(stop_deadline)
            .run(async move {
                let _ = stopped.await;
            }),
    );
    Portal {
        address,
        client: reqwest::Client::new(),
        log,
        store,
        stop: Some(stop),
        host,
        engine: handed.await.unwrap(),
    }
}

/// A reply body as JSON; `Null` when empty. A body that is not JSON fails
/// the test and shows the body.
pub fn json_body(path: &str, text: &str) -> Value {
    if text.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(text).unwrap_or_else(|error| panic!("{path}: {error}: {text}"))
}

impl Portal {
    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("http://{}{path}", self.address))
    }

    pub async fn json(&self, method: Method, path: &str, body: Value) -> (StatusCode, Value) {
        let response = self.request(method, path).json(&body).send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        (status, json_body(path, &text))
    }

    pub async fn get(&self, path: &str) -> (StatusCode, Value) {
        let response = self.request(Method::GET, path).send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        (status, json_body(path, &text))
    }

    pub async fn state(&self) -> Value {
        let response = self
            .request(Method::GET, "/api/v1/state")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    pub async fn target(&self, target: &str) -> (StatusCode, Value) {
        self.json(Method::PUT, "/api/v1/target", json!({ "target": target }))
            .await
    }

    pub async fn shutdown(mut self) -> prysm_ports::Result<()> {
        let _ = self.stop.take().unwrap().send(());
        timeout(LIMIT, self.host).await.unwrap().unwrap()
    }
}

pub fn with_output(output: FakeOutputSettings) -> Settings {
    Settings {
        output: Some(AdapterSettings::new("fake", &output)),
        ..fakes::settings()
    }
}

/// Every response, asset or API, success or failure; `cache` is how long it
/// may be kept.
pub fn assert_secure_kept(response: &reqwest::Response, cache: &str) {
    let headers = response.headers();
    assert_eq!(headers[header::CACHE_CONTROL], cache);
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'self'; frame-ancestors 'none'"
    );
    assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
}

/// Everything but the bundle is never stored.
pub fn assert_secure(response: &reqwest::Response) {
    assert_secure_kept(response, "no-store");
}

/// Read server-sent events until one satisfies `done`.
pub async fn next_state(
    events: &mut (impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin),
    buffer: &mut String,
    done: impl Fn(&Value) -> bool,
) -> Value {
    loop {
        while let Some(end) = buffer.find("\n\n") {
            let event: String = buffer.drain(..end + 2).collect();
            if let Some(data) = event.strip_prefix("data: ") {
                let state: Value = serde_json::from_str(data.trim_end()).unwrap();
                if done(&state) {
                    return state;
                }
            }
        }
        let chunk = events.next().await.expect("stream ended").unwrap();
        buffer.push_str(std::str::from_utf8(&chunk).unwrap());
    }
}

pub async fn preview_stream(portal: &Portal) -> reqwest::Response {
    portal
        .request(Method::GET, "/api/v1/preview/stream")
        .send()
        .await
        .unwrap()
}

/// Wait until the engine sees `count` preview viewers.
pub async fn viewers(portal: &Portal, count: usize) {
    eventually_async("the preview viewers", LIMIT, || {
        (portal.engine.preview_viewers() == count).then_some(())
    })
    .await;
}
