//! The portal against a real engine with fake adapters, over loopback HTTP.
use futures::StreamExt;
use prysm_ports::{AdapterSettings, Settings};
use prysm_runtime::Prysm;
use prysm_runtime::fakes::{
    self, CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_web_portal::{Access, FrontendAsset, WebPortal};
use reqwest::{Method, RequestBuilder, StatusCode, header};
use serde_json::{Value, json};
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const ASSETS: &[FrontendAsset] = &[
    FrontendAsset {
        path: "/index.html",
        content_type: "text/html; charset=utf-8",
        bytes: b"<!doctype html><title>Prysm</title>",
    },
    FrontendAsset {
        path: "/assets/app.js",
        content_type: "text/javascript",
        bytes: b"console.log('prysm')",
    },
];
const LIMIT: Duration = Duration::from_secs(10);

struct Portal {
    address: SocketAddr,
    client: reqwest::Client,
    log: CallLog,
    store: MemoryStore,
    stop: Option<oneshot::Sender<()>>,
    host: JoinHandle<prysm_ports::Result<()>>,
}

async fn start(settings: Settings, stop_deadline: Duration) -> Portal {
    start_with(settings, stop_deadline, Access::SameOrigin).await
}

async fn start_with(settings: Settings, stop_deadline: Duration, access: Access) -> Portal {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let portal = WebPortal::from_listener(listener, ASSETS, access).unwrap();
    let log = CallLog::new();
    let store = MemoryStore::with(settings);
    let (stop, stopped) = oneshot::channel::<()>();
    let host = tokio::spawn(
        Prysm::builder(store.clone())
            .source(FakeSources(log.clone()))
            .output(FakeOutputs(log.clone()))
            .surface(portal)
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
    }
}

impl Portal {
    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("http://{}{path}", self.address))
    }

    async fn json(&self, method: Method, path: &str, body: Value) -> (StatusCode, Value) {
        let response = self.request(method, path).json(&body).send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn state(&self) -> Value {
        let response = self
            .request(Method::GET, "/api/v1/state")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    async fn target(&self, target: &str) -> (StatusCode, Value) {
        self.json(Method::PUT, "/api/v1/target", json!({ "target": target }))
            .await
    }

    async fn shutdown(mut self) -> prysm_ports::Result<()> {
        let _ = self.stop.take().unwrap().send(());
        timeout(LIMIT, self.host).await.unwrap().unwrap()
    }
}

async fn bounded<F: Future>(test: F) -> F::Output {
    timeout(LIMIT, test).await.expect("test timed out")
}

fn with_output(output: FakeOutputSettings) -> Settings {
    Settings {
        output: Some(AdapterSettings::new("fake", &output)),
        ..fakes::settings()
    }
}

#[tokio::test]
async fn construction_accepts_any_address_and_rejects_bad_assets_and_origins() {
    let field = |result: prysm_ports::Result<WebPortal>| match result {
        Err(prysm_ports::Error::Invalid(invalid)) => invalid.field,
        other => panic!("expected a validation error, got {other:?}"),
    };
    let lan: SocketAddr = "0.0.0.0:0".parse().unwrap();
    WebPortal::new(lan, ASSETS, Access::SameOrigin).unwrap();
    WebPortal::new(lan, ASSETS, Access::AnyOrigin).unwrap();
    let origins = |origins: &[&str]| Access::Origins(origins.iter().map(|o| (*o).into()).collect());
    WebPortal::new(
        lan,
        ASSETS,
        origins(&[
            "http://192.168.1.20:3000",
            "HTTPS://TV.example:443",
            "app://x",
        ]),
    )
    .unwrap();
    for origin in [
        "null",
        "*",
        "192.168.1.20",
        "http://tv.example/",
        "http://tv.example/path",
        "http://user@tv.example",
        "http://tv.example?x",
        "",
    ] {
        assert_eq!(
            field(WebPortal::new(lan, ASSETS, origins(&[origin]))),
            "portal.access",
            "{origin}"
        );
    }
    for path in ["/api/v1/state", "relative", "/../x", "/a%2e", "/index.html"] {
        let assets: &'static [FrontendAsset] = Box::leak(Box::new([
            ASSETS[0].clone(),
            FrontendAsset {
                path: Box::leak(path.to_owned().into_boxed_str()),
                ..ASSETS[1].clone()
            },
        ]));
        assert_eq!(
            field(WebPortal::new(lan, assets, Access::SameOrigin)),
            "portal.assets"
        );
    }
}

#[tokio::test]
async fn asset_serving_and_unauthenticated_api() {
    bounded(async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let url = |path: &str| format!("http://{}{path}", portal.address);
        let client = &portal.client;

        let page = client.get(url("/")).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(
            page.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(page.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(page.text().await.unwrap().contains("Prysm"));
        let script = client.get(url("/assets/app.js")).send().await.unwrap();
        assert_eq!(script.headers()[header::CONTENT_TYPE], "text/javascript");
        assert_eq!(
            client.get(url("/missing")).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        // Any Host is served: the portal is reached by IP or LAN name.
        let named = client
            .get(url("/api/v1/state"))
            .header(header::HOST, "prysm.lan:8787")
            .send()
            .await
            .unwrap();
        assert_eq!(named.status(), StatusCode::OK);
        assert_eq!(named.headers()[header::VARY], "origin");

        let unknown = portal
            .request(Method::GET, "/api/v1/nothing")
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let method = portal
            .request(Method::POST, "/api/v1/target")
            .send()
            .await
            .unwrap();
        assert_eq!(method.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            method.json::<Value>().await.unwrap()["kind"],
            "method_not_allowed"
        );
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn foreign_origins_are_refused_without_running_the_command() {
    bounded(async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let put = |origin: Option<&str>, host: Option<&str>| {
            let mut request = portal
                .request(Method::PUT, "/api/v1/target")
                .json(&json!({ "target": "processed" }));
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            if let Some(host) = host {
                request = request.header(header::HOST, host);
            }
            request.send()
        };

        for origin in ["http://evil.example", "null", "https://127.0.0.1"] {
            let refused = put(Some(origin), None).await.unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{origin}");
            assert!(
                !refused
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            );
            assert_eq!(
                refused.json::<Value>().await.unwrap(),
                json!({ "kind": "forbidden", "message": "origin denied" })
            );
        }
        let stream = portal
            .request(Method::GET, "/api/v1/state/stream")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::FORBIDDEN);
        let preflight = portal
            .request(Method::OPTIONS, "/api/v1/target")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::FORBIDDEN);
        assert_eq!(portal.state().await["target"], "off");

        // The portal's own pages, by IP or by name, and clients without Origin.
        let own = format!("http://{}", portal.address);
        let same = put(Some(&own), None).await.unwrap();
        assert_eq!(same.status(), StatusCode::NO_CONTENT);
        assert!(
            !same
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
        let named = put(Some("http://Prysm.lan"), Some("prysm.lan:80"))
            .await
            .unwrap();
        assert_eq!(named.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            put(None, None).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn allowed_origins_get_cors() {
    bounded(async {
        let tv = "http://192.168.1.30:3000";
        let access = Access::Origins(vec!["HTTP://192.168.1.30:3000".into()]);
        let portal = start_with(fakes::settings(), Duration::from_secs(5), access).await;

        let preflight = portal
            .request(Method::OPTIONS, "/api/v1/target")
            .header(header::ORIGIN, tv)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
            .header("access-control-request-private-network", "true")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        let headers = preflight.headers();
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], tv);
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_METHODS], "GET, PUT");
        assert_eq!(
            headers[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "content-type"
        );
        assert_eq!(headers["access-control-allow-private-network"], "true");
        assert_eq!(headers[header::VARY], "origin");

        let put = portal
            .request(Method::PUT, "/api/v1/target")
            .header(header::ORIGIN, tv)
            .json(&json!({ "target": "processed" }))
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::NO_CONTENT);
        assert_eq!(put.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], tv);
        let stream = portal
            .request(Method::GET, "/api/v1/state/stream")
            .header(header::ORIGIN, tv)
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::OK);
        assert_eq!(stream.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], tv);
        drop(stream);

        let other = portal
            .request(Method::GET, "/api/v1/state")
            .header(header::ORIGIN, "http://192.168.1.31:3000")
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::FORBIDDEN);
        portal.shutdown().await.unwrap();

        let portal = start_with(fakes::settings(), Duration::from_secs(5), Access::AnyOrigin).await;
        for origin in ["null", "http://anything.example"] {
            let state = portal
                .request(Method::GET, "/api/v1/state")
                .header(header::ORIGIN, origin)
                .send()
                .await
                .unwrap();
            assert_eq!(state.status(), StatusCode::OK);
            assert_eq!(state.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
        }
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn state_read_and_set_target() {
    bounded(async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let state = portal.state().await;
        assert_eq!(state["target"], "off");
        assert_eq!(state["session"], json!({ "state": "stopped" }));
        assert_eq!(state["revision"], 0);
        assert_eq!(state["settings"]["capture"]["kind"], "fake");
        assert_eq!(state["failure"], Value::Null);

        let (status, _) = portal.target("processed").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let state = portal.state().await;
        assert_eq!(state["target"], "processed");
        assert_eq!(state["session"]["state"], "running");
        assert!(portal.log.wait_for("output.write:", 1).await);

        let (status, _) = portal.target("off").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(portal.state().await["session"]["state"], "stopped");
        assert_eq!(portal.log.count("output.idle::Off"), 1);

        let (status, body) = portal.target("sideways").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["kind"], "bad_request");
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn adapter_failures_are_bad_gateway_and_missing_settings_are_invalid() {
    bounded(async {
        let failing = with_output(FakeOutputSettings {
            fail_open: true,
            ..FakeOutputSettings::default()
        });
        let portal = start(failing, Duration::from_secs(5)).await;
        let (status, body) = portal.target("processed").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body,
            json!({ "kind": "adapter", "message": "fake output cannot open" })
        );
        assert_eq!(portal.state().await["failure"]["kind"], "adapter");
        portal.shutdown().await.unwrap();

        let portal = start(Settings::default(), Duration::from_secs(5)).await;
        let (status, body) = portal.target("processed").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "capture");
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn settings_replace_succeeds_conflicts_and_validates() {
    bounded(async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let mut settings = portal.state().await["settings"].clone();
        settings["processing"]["brightness_percent"] = json!(40);
        let replace = |expected: u64, settings: Value| {
            portal.json(
                Method::PUT,
                "/api/v1/settings",
                json!({ "expected": expected, "settings": settings }),
            )
        };

        let (status, body) = replace(0, settings.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "revision": 1 }));
        assert_eq!(
            portal.store.saved().unwrap().processing.brightness_percent,
            40
        );
        let state = portal.state().await;
        assert_eq!(state["revision"], 1);
        assert_eq!(state["settings"]["processing"]["brightness_percent"], 40);

        let (status, body) = replace(0, settings.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["kind"], "stale_revision");
        assert_eq!(body["current"], 1);

        let mut invalid = settings.clone();
        invalid["capture"] = serde_json::to_value(AdapterSettings::new(
            "fake",
            &FakeSourceSettings {
                invalid: true,
                ..FakeSourceSettings::default()
            },
        ))
        .unwrap();
        let (status, body) = replace(1, invalid).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body,
            json!({ "kind": "invalid", "message": "fake rejects it", "field": "capture.invalid" })
        );

        let mut range = settings.clone();
        range["processing"]["spatial_reach_percent"] = json!(0);
        let (status, body) = replace(1, range).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "processing.spatial_reach_percent");

        let mut unknown = settings.clone();
        unknown["output"] = json!({ "kind": "laser" });
        let (status, body) = replace(1, unknown).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "output.kind");

        let mut undecodable = settings.clone();
        undecodable["output"] = json!({ "kind": "fake", "fail_after": "soon" });
        let (status, body) = replace(1, undecodable).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .starts_with("cannot decode `fake` settings:")
        );
        assert_eq!(portal.state().await["revision"], 1);

        let plain = portal
            .request(Method::PUT, "/api/v1/settings")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(plain.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let large = portal
            .request(Method::PUT, "/api/v1/settings")
            .header(header::CONTENT_TYPE, "application/json")
            .body(vec![b' '; 1024 * 1024 + 1])
            .send()
            .await
            .unwrap();
        assert_eq!(large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// Read server-sent events until one satisfies `done`.
async fn next_state(
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

#[tokio::test]
async fn state_stream_delivers_changes_and_ends_on_shutdown() {
    bounded(async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let response = portal
            .request(Method::GET, "/api/v1/state/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let mut events = response.bytes_stream();
        let mut buffer = String::new();
        let first = next_state(&mut events, &mut buffer, |_| true).await;
        assert_eq!(first["session"]["state"], "stopped");

        let (status, _) = portal.target("processed").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let running = next_state(&mut events, &mut buffer, |s| {
            s["session"]["state"] == "running"
        })
        .await;
        assert_eq!(running["target"], "processed");

        portal.shutdown().await.unwrap();
        // The stream ends instead of hanging once the portal stops.
        while let Some(chunk) = events.next().await {
            if chunk.is_err() {
                break;
            }
        }
    })
    .await;
}

#[tokio::test]
async fn state_streams_are_limited() {
    bounded(async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let mut open = Vec::new();
        for _ in 0..8 {
            let response = portal
                .request(Method::GET, "/api/v1/state/stream")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            open.push(response);
        }
        let refused = portal
            .request(Method::GET, "/api/v1/state/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.json::<Value>().await.unwrap()["kind"], "busy");
        drop(open);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn faulted_engine_is_unavailable() {
    bounded(async {
        let slow_close = Settings {
            capture: Some(AdapterSettings::new(
                "fake",
                &FakeSourceSettings {
                    close_delay_ms: 500,
                    ..FakeSourceSettings::default()
                },
            )),
            ..fakes::settings()
        };
        let portal = start(slow_close, Duration::from_millis(50)).await;
        assert_eq!(portal.target("processed").await.0, StatusCode::NO_CONTENT);
        let (status, body) = portal.target("off").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["kind"], "faulted");
        let (status, body) = portal.target("processed").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["kind"], "faulted");
        let state = portal.state().await;
        assert_eq!(
            state["session"],
            json!({ "state": "faulted", "phase": "closing_source" })
        );
        // A faulted engine cannot shut down cleanly; the portal still stops.
        assert!(portal.shutdown().await.is_err());
    })
    .await;
}

#[tokio::test]
async fn shutdown_closes_the_listener_promptly() {
    bounded(async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let address = portal.address;
        assert_eq!(portal.state().await["target"], "off");
        let started = std::time::Instant::now();
        portal.shutdown().await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    })
    .await;
}
