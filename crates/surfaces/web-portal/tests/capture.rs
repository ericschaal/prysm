//! The capture stream path against a real engine whose source is the remote
//! receiver: a sender (the TV's slim worker) streams through the portal.
use prysm_capture_remote::{
    RemoteReceiver, RemoteSender, RemoteSettings, StreamEncoding, WireError,
};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{AdapterSettings, FrameSink, LightingTarget, Modes, Settings};
use prysm_runtime::fakes::{CallLog, FakeOutputSettings, FakeOutputs, MemoryStore};
use prysm_runtime::{EngineHandle, Prysm, Shutdown};
use prysm_testkit::{eventually, within_async};
use prysm_web_portal::{Access, WebPortal};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::{HeaderValue, Uri};

const LIMIT: Duration = Duration::from_secs(10);

const RAW: RemoteSettings = RemoteSettings {
    width: 16,
    height: 8,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::Raw,
};

struct Host {
    address: SocketAddr,
    engine: EngineHandle,
    stop: Option<oneshot::Sender<()>>,
    host: JoinHandle<prysm_ports::Result<()>>,
}

/// A headless-like host: remote capture, a fake output, the portal with
/// (or without) the capture stream path.
async fn start(capture: bool) -> Host {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let remote = RemoteReceiver::default();
    let mut portal = WebPortal::from_listener(listener, &[], Access::SameOrigin).unwrap();
    if capture {
        portal = portal.capture_stream(remote.incoming());
    }
    let settings = Settings {
        capture: Some(AdapterSettings::new("remote", &RAW)),
        output: Some(AdapterSettings::new(
            "fake",
            &FakeOutputSettings {
                name: "lights".into(),
                ..Default::default()
            },
        )),
        ..Settings::default()
    };
    let log = CallLog::new();
    let (stop, stopped) = oneshot::channel::<()>();
    let (engine, handed) = oneshot::channel::<EngineHandle>();
    let host = tokio::spawn(
        Prysm::builder(MemoryStore::with(settings))
            .source(remote)
            .output(FakeOutputs(log))
            .surface(portal)
            .surface(move |handle: EngineHandle, shutdown: Shutdown| async move {
                let _ = engine.send(handle);
                shutdown.requested().await;
                Ok(())
            })
            .run(async move {
                let _ = stopped.await;
            }),
    );
    Host {
        address,
        engine: handed.await.unwrap(),
        stop: Some(stop),
        host,
    }
}

impl Host {
    fn url(&self) -> Uri {
        format!("ws://{}/api/v1/capture/stream", self.address)
            .parse()
            .unwrap()
    }

    /// Connect a sender off the async runtime, as the TV's worker does.
    async fn connect(&self) -> prysm_ports::Result<RemoteSender> {
        let (url, address) = (self.url(), self.address);
        let offer = prysm_capture_remote::testkit::offer();
        tokio::task::spawn_blocking(move || RemoteSender::connect(&url, address, &offer))
            .await
            .unwrap()
    }

    async fn shutdown(mut self) {
        let _ = self.stop.take().unwrap().send(());
        timeout(LIMIT, self.host).await.unwrap().unwrap().unwrap();
    }
}

fn frame() -> Frame {
    Frame::fill(9, 16, 8, PixelFormat::RGB24).unwrap()
}

/// Send frames until the engine counts `count` received.
async fn deliver(sender: RemoteSender, engine: &EngineHandle, count: u64) -> RemoteSender {
    let start = engine.stats().frames_received;
    let mut sender = Some(sender);
    let deadline = Instant::now() + LIMIT;
    while engine.stats().frames_received < start + count {
        assert!(Instant::now() < deadline, "frames did not arrive");
        let mut moved = sender.take().unwrap();
        moved = tokio::task::spawn_blocking(move || {
            moved.send(&frame()).unwrap();
            moved
        })
        .await
        .unwrap();
        sender = Some(moved);
    }
    sender.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sender_streams_through_the_portal_once_lighting_runs() {
    within_async(LIMIT, async {
        let host = start(true).await;
        let remote = AdapterSettings::new("remote", &RAW);
        let modes = || host.engine.capture_modes(remote.clone());
        assert_eq!(modes().await.unwrap(), Modes::Unknown);
        // Lighting is off: nobody receives; the sender is told and retries.
        let error = host.connect().await.unwrap_err();
        assert_eq!(
            error.cause::<WireError>(),
            Some(&WireError::Unavailable(503)),
            "{error}"
        );
        // Its refused attempt carried its offer.
        assert_eq!(
            modes().await.unwrap(),
            prysm_capture_remote::testkit::offer().modes(StreamEncoding::Raw)
        );

        host.engine
            .set_target(LightingTarget::ScreenSync)
            .await
            .unwrap();
        // The target is set once the session has opened its source, which
        // receives from then on.
        let sender = host.connect().await.unwrap();
        assert_eq!(sender.request(), RAW);
        let first = deliver(sender, &host.engine, 3).await;

        // A second sender replaces the first, whose stream ends.
        let second = host.connect().await.unwrap();
        deliver(second, &host.engine, 3).await;
        let ended = tokio::task::spawn_blocking(move || {
            let mut first = first;
            eventually("the replaced sender to stop", LIMIT, || first.tick().err());
        });
        ended.await.unwrap();
        host.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browsers_from_other_sites_cannot_open_it_and_without_a_source_it_is_absent() {
    within_async(LIMIT, async {
        let host = start(true).await;
        host.engine
            .set_target(LightingTarget::ScreenSync)
            .await
            .unwrap();
        // A page on another site: browsers always send its origin.
        let mut request = host.url().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", HeaderValue::from_static("http://evil.example"));
        let address = host.address;
        let refused = tokio::task::spawn_blocking(move || {
            let socket = std::net::TcpStream::connect(address).unwrap();
            tungstenite::client(request, socket).unwrap_err()
        })
        .await
        .unwrap();
        let tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) = refused
        else {
            panic!("{refused:?}");
        };
        assert_eq!(response.status(), 403);
        // A sender of another version: refused, naming the version here.
        let mut request = host.url().into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("prysm-capture.v3"),
        );
        let refused = tokio::task::spawn_blocking(move || {
            let socket = std::net::TcpStream::connect(address).unwrap();
            tungstenite::client(request, socket).unwrap_err()
        })
        .await
        .unwrap();
        let tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) = refused
        else {
            panic!("{refused:?}");
        };
        assert_eq!(response.status(), 426);
        assert_eq!(
            response.headers()["sec-websocket-protocol"],
            prysm_ports::CAPTURE_PROTOCOL
        );
        // A plain GET is not an upgrade.
        let plain = reqwest::get(format!("http://{address}/api/v1/capture/stream"))
            .await
            .unwrap();
        assert_eq!(plain.status(), 400);
        host.shutdown().await;

        // A portal without the capture stream: the path does not exist.
        let host = start(false).await;
        let error = host.connect().await.unwrap_err();
        assert_eq!(
            error.cause::<WireError>(),
            Some(&WireError::Unavailable(404)),
            "{error}"
        );
        host.shutdown().await;
    })
    .await;
}
