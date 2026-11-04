use super::*;
use crate::{
    control::{Controller, controller::ControllerHandle},
    preview::Preview,
    test_support,
};
use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message as ServerMessage, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use prysm_api::App;
use prysm_contract::{OperationStatus, Source};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
};
use tokio::{
    sync::{broadcast, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

struct DeviceState {
    document: Mutex<serde_json::Value>,
    requests: AtomicUsize,
    attempts: AtomicUsize,
    connections: AtomicUsize,
    closed: AtomicUsize,
    heartbeats: AtomicUsize,
    heartbeat_acks: AtomicUsize,
    websocket_enabled: AtomicBool,
    answer_heartbeats: AtomicBool,
    http_gates: Mutex<VecDeque<oneshot::Receiver<()>>>,
    messages: broadcast::Sender<ServerMessage>,
    shutdown: CancellationToken,
}
struct Device {
    state: Arc<DeviceState>,
    url: reqwest::Url,
    server: JoinHandle<()>,
}
impl Device {
    async fn start() -> Self {
        let state = Arc::new(DeviceState {
            document: Mutex::new(serde_json::json!({
                "state": {"on":false,"lor":2}, "info":{"live":false,"lm":null}
            })),
            requests: AtomicUsize::new(0),
            attempts: AtomicUsize::new(0),
            connections: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            heartbeats: AtomicUsize::new(0),
            heartbeat_acks: AtomicUsize::new(0),
            websocket_enabled: AtomicBool::new(true),
            answer_heartbeats: AtomicBool::new(true),
            http_gates: Mutex::new(VecDeque::new()),
            messages: broadcast::channel(16).0,
            shutdown: CancellationToken::new(),
        });
        let router = Router::new()
            .route(
                "/json/si",
                get(|State(state): State<Arc<DeviceState>>| async move {
                    state.requests.fetch_add(1, Relaxed);
                    let document = state.document.lock().unwrap().clone();
                    let gate = state.http_gates.lock().unwrap().pop_front();
                    if let Some(gate) = gate {
                        gate.await.unwrap();
                    }
                    Json(document)
                }),
            )
            .route("/ws", get(websocket))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let shutdown = state.shutdown.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        Self { state, url, server }
    }
    fn push(&self, text: &str) {
        self.state
            .messages
            .send(ServerMessage::Text(text.to_owned().into()))
            .unwrap();
    }
    fn power(&self, on: bool, push: bool) {
        let mut document = self.state.document.lock().unwrap();
        document["state"]["on"] = on.into();
        if push {
            self.push(&document.to_string());
        }
    }
    async fn stop(self) {
        self.state.shutdown.cancel();
        self.server.await.unwrap();
    }
}
async fn websocket(State(state): State<Arc<DeviceState>>, ws: WebSocketUpgrade) -> Response {
    state.attempts.fetch_add(1, Relaxed);
    if !state.websocket_enabled.load(Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    ws.on_upgrade(move |socket| connection(socket, state))
}
async fn connection(mut socket: WebSocket, state: Arc<DeviceState>) {
    let mut messages = state.messages.subscribe();
    let initial = state.document.lock().unwrap().to_string();
    socket
        .send(ServerMessage::Text(initial.into()))
        .await
        .unwrap();
    state.connections.fetch_add(1, Relaxed);
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => break,
            message = messages.recv() => {
                let message = message.unwrap();
                let close = matches!(message, ServerMessage::Close(_));
                if socket.send(message).await.is_err() || close { break; }
            }
            message = socket.recv() => match message {
                Some(Ok(ServerMessage::Text(text))) if text == "p" => {
                    if state.answer_heartbeats.load(Relaxed)
                        && (socket.send(ServerMessage::Text("pong".into())).await.is_err()
                            || socket.send(ServerMessage::Ping(Vec::new().into())).await.is_err()) {
                        break;
                    }
                    state.heartbeats.fetch_add(1, Relaxed);
                }
                Some(Ok(ServerMessage::Pong(_))) => { state.heartbeat_acks.fetch_add(1, Relaxed); }
                Some(Ok(ServerMessage::Ping(_))) => {}
                _ => break,
            }
        }
    }
    state.closed.fetch_add(1, Relaxed);
}

struct Control {
    app: App,
    controller: ControllerHandle,
    _directory: tempfile::TempDir,
}
impl Control {
    async fn start(device: &Device) -> Self {
        device.state.websocket_enabled.store(false, Relaxed);
        let directory = tempfile::tempdir().unwrap();
        let mut config = test_support::config();
        config.source = Source::Wled;
        let prysm_contract::Output::Wled { url, .. } = &mut config.output;
        *url = device.url.clone();
        let store = test_support::store(&directory, &config);
        let (controller, app) = Controller::start(
            store,
            Preview::new().publisher(),
            prysm_capture::WebosReceiver::new().unwrap(),
        );
        let control = Self {
            app,
            controller,
            _directory: directory,
        };
        until(|| {
            control.app.state().unwrap().observed.is_some()
                && device.state.attempts.load(Relaxed) == 1
        })
        .await;
        // Complete the initial HTTP request before advancing virtual time, then
        // distinguish the first pushed snapshot from that HTTP snapshot.
        device.state.document.lock().unwrap()["state"]["lor"] = 0.into();
        device.state.websocket_enabled.store(true, Relaxed);
        until(|| {
            device.state.connections.load(Relaxed) == 1
                && control
                    .app
                    .state()
                    .unwrap()
                    .observed
                    .as_ref()
                    .is_some_and(|state| state.live_override == 0)
        })
        .await;
        control
    }
    fn power(&self) -> Option<bool> {
        self.app
            .state()
            .unwrap()
            .observed
            .as_ref()
            .map(|observed| observed.power)
    }
    async fn stop(self) {
        self.controller.shutdown().await.unwrap();
    }
}
async fn until(mut predicate: impl FnMut() -> bool) {
    // Keep paused-time tests runnable while loopback I/O is serviced.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "condition did not become true"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn pushed_status_reaches_controller_without_an_http_poll() {
    let device = Device::start().await;
    let control = Control::start(&device).await;
    device.push(r#"{"success":true}"#);
    device.push("pong");
    device.push(r#"{"state":{"on":true,"lor":0},"info":{"live":true,"lm":"DDP"}}"#);
    until(|| control.power() == Some(true)).await;
    let observed = control.app.state().unwrap().observed.clone().unwrap();
    assert!(observed.realtime);
    assert_eq!(observed.realtime_source.as_deref(), Some("DDP"));
    assert_eq!(observed.live_override, 0);
    assert_eq!(device.state.requests.load(Relaxed), 1);
    control.stop().await;
    until(|| device.state.closed.load(Relaxed) == 1).await;
    device.stop().await;
}

#[tokio::test]
async fn reconnects_after_close_and_after_rejected_handshakes() {
    let device = Device::start().await;
    let control = Control::start(&device).await;
    device.state.websocket_enabled.store(false, Relaxed);
    let attempts = device.state.attempts.load(Relaxed);
    device
        .state
        .messages
        .send(ServerMessage::Close(None))
        .unwrap();
    until(|| device.state.attempts.load(Relaxed) > attempts).await;
    device.power(true, false);
    device.state.websocket_enabled.store(true, Relaxed);
    until(|| control.power() == Some(true) && device.state.connections.load(Relaxed) == 2).await;
    assert_eq!(device.state.requests.load(Relaxed), 1);
    control.stop().await;
    device.stop().await;
}

#[tokio::test]
async fn periodic_http_resync_recovers_a_missed_push_and_heartbeats_keep_socket_open() {
    let device = Device::start().await;
    let control = Control::start(&device).await;
    tokio::time::pause();
    device.power(true, false);
    tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
    until(|| device.state.heartbeat_acks.load(Relaxed) == 1).await;
    assert_eq!(control.power(), Some(false));
    assert_eq!(device.state.requests.load(Relaxed), 1);
    tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
    until(|| control.power() == Some(true) && device.state.requests.load(Relaxed) == 2).await;
    assert_eq!(device.state.connections.load(Relaxed), 1);
    device.state.websocket_enabled.store(false, Relaxed);
    device
        .state
        .messages
        .send(ServerMessage::Close(None))
        .unwrap();
    until(|| device.state.closed.load(Relaxed) == 1).await;
    device.power(false, false);
    tokio::time::advance(RESYNC_INTERVAL).await;
    until(|| control.power() == Some(false) && device.state.requests.load(Relaxed) == 3).await;
    control.stop().await;
    device.stop().await;
}

#[tokio::test]
async fn stalled_socket_reconnects_after_heartbeat_timeout() {
    let device = Device::start().await;
    let control = Control::start(&device).await;
    tokio::time::pause();
    device.state.answer_heartbeats.store(false, Relaxed);
    tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
    until(|| device.state.heartbeats.load(Relaxed) == 1).await;
    tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
    until(|| device.state.closed.load(Relaxed) == 1).await;
    device.state.answer_heartbeats.store(true, Relaxed);
    tokio::time::advance(Duration::from_millis(1001)).await;
    until(|| device.state.connections.load(Relaxed) == 2).await;
    device.power(true, true);
    until(|| control.power() == Some(true)).await;
    control.stop().await;
    device.stop().await;
}

#[tokio::test]
async fn delayed_http_response_cannot_replace_a_newer_pushed_status() {
    let device = Device::start().await;
    let control = Control::start(&device).await;
    let (release, gate) = oneshot::channel();
    device.state.http_gates.lock().unwrap().push_back(gate);
    tokio::time::pause();
    tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
    until(|| device.state.heartbeat_acks.load(Relaxed) == 1).await;
    tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
    until(|| device.state.requests.load(Relaxed) == 2).await;
    device.power(true, true);
    until(|| control.power() == Some(true)).await;
    release.send(()).unwrap();
    tokio::time::resume();
    sleep(Duration::from_millis(50)).await;
    assert_eq!(control.power(), Some(true));
    control.stop().await;
    device.stop().await;
}

#[tokio::test]
async fn delayed_operation_snapshot_cannot_replace_a_newer_pushed_status() {
    let device = Device::start().await;
    let control = Control::start(&device).await;
    let (release_poll, poll_gate) = oneshot::channel();
    let (release_operation, operation_gate) = oneshot::channel();
    device
        .state
        .http_gates
        .lock()
        .unwrap()
        .extend([poll_gate, operation_gate]);
    let mut config = control
        .app
        .settings()
        .settings
        .config(control.app.settings().revision)
        .unwrap();
    config.processing.brightness_percent = 42.try_into().unwrap();
    let response = control
        .app
        .update_settings(test_support::update(&control.app, &config))
        .await;
    response.unwrap();
    until(|| {
        device.state.requests.load(Relaxed) == 3 && device.state.connections.load(Relaxed) == 2
    })
    .await;
    device.power(true, true);
    until(|| control.power() == Some(true)).await;
    release_poll.send(()).unwrap();
    release_operation.send(()).unwrap();
    until(|| {
        matches!(
            control.app.state().unwrap().operation,
            OperationStatus::Succeeded { .. }
        )
    })
    .await;
    assert_eq!(control.power(), Some(true));
    control.stop().await;
    device.stop().await;
}

#[tokio::test]
async fn configuration_change_retires_the_old_device_connection() {
    let old = Device::start().await;
    let new = Device::start().await;
    let control = Control::start(&old).await;
    new.power(true, false);
    let mut config = control
        .app
        .settings()
        .settings
        .config(control.app.settings().revision)
        .unwrap();
    let prysm_contract::Output::Wled { url, .. } = &mut config.output;
    *url = new.url.clone();
    let response = control
        .app
        .update_settings(test_support::update(&control.app, &config))
        .await;
    let saved = response.unwrap();
    until(|| {
        new.state.connections.load(Relaxed) == 1
            && control.power() == Some(true)
            && old.state.closed.load(Relaxed) == 1
    })
    .await;
    assert_eq!(control.app.state().unwrap().config_revision, saved.revision);
    new.power(false, true);
    until(|| control.power() == Some(false)).await;
    control.stop().await;
    old.stop().await;
    new.stop().await;
}

#[tokio::test]
async fn invalid_status_reconnects_without_publishing_unvalidated_fields() {
    let device = Device::start().await;
    let control = Control::start(&device).await;
    device.push(r#"{"state":{"on":true},"info":{"live":true}}"#);
    until(|| device.state.connections.load(Relaxed) == 2).await;
    assert_eq!(control.power(), Some(false));
    control.stop().await;
    device.stop().await;
}

#[tokio::test]
#[ignore = "requires PRYSM_TEST_WLED_URL pointing to a real WLED device; read-only"]
async fn live_wled_snapshot_and_heartbeat() {
    let url = std::env::var("PRYSM_TEST_WLED_URL")
        .unwrap()
        .parse()
        .unwrap();
    let mut snapshots = 0;
    let result = timeout(
        IDLE_TIMEOUT + Duration::from_secs(1),
        websocket_session(&url, |_| snapshots += 1),
    )
    .await;
    assert!(result.is_err(), "live WebSocket disconnected: {result:?}");
    assert!(snapshots > 0, "device did not send a valid initial status");
}
