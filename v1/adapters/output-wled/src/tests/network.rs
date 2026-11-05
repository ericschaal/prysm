use super::*;
use prysm_application::{Application, ApplicationHandle, policy::V1Policy};
use prysm_contract::{
    application::{
        ApplicationCommand, ApplicationControl, ApplicationSnapshot, LightingTarget,
        OperationStatus, Request, SessionState, Settings,
    },
    capture::{
        CaptureCapabilities, CaptureControl, CaptureStart, CaptureStarted, CaptureState,
        CaptureStopped, CapturedFrame, FrameSource,
    },
    identity::OperationId,
    observation::{Cursor, Versioned},
    storage::SettingsRepository,
};
use prysm_core::{Frame, PixelFormat, SmoothingSeconds};
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_processor::PrysmProcessor;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
    sync::watch,
};

struct HttpCall {
    json: Value,
    response: oneshot::Sender<Option<String>>,
}
impl HttpCall {
    fn ok(self) {
        self.response
            .send(Some(r#"{"success":true}"#.into()))
            .unwrap();
    }
}
struct Device {
    udp: UdpSocket,
    calls: mpsc::UnboundedReceiver<HttpCall>,
    shutdown: oneshot::Sender<()>,
    server: JoinHandle<()>,
}
impl Device {
    async fn new() -> (Self, transport::Network) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let network = transport::Network::local(
            listener.local_addr().unwrap().port(),
            udp.local_addr().unwrap().port(),
        );
        let (sender, calls) = mpsc::unbounded_channel();
        let (shutdown, mut stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = tokio::select! {
                    _ = &mut stopped => break,
                    connection = listener.accept() => connection.unwrap(),
                };
                let mut bytes = Vec::new();
                let end = loop {
                    let mut chunk = [0; 1024];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break end + 4;
                    }
                    assert!(bytes.len() < 8192);
                };
                let header = std::str::from_utf8(&bytes[..end]).unwrap();
                assert!(header.starts_with("POST /json/state HTTP/1.1\r\n"));
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < end + length {
                    let mut chunk = [0; 1024];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let json = serde_json::from_slice(&bytes[end..end + length]).unwrap();
                let (response, reply) = oneshot::channel();
                sender.send(HttpCall { json, response }).unwrap();
                if let Some(body) = reply.await.unwrap() {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                }
            }
        });
        (
            Self {
                udp,
                calls,
                shutdown,
                server,
            },
            network,
        )
    }
    async fn call(&mut self, expected: Value) -> HttpCall {
        let call = within(self.calls.recv()).await.unwrap();
        assert_eq!(call.json, expected);
        call
    }
    async fn realtime(&mut self) {
        self.call(json!({"live":false})).await.ok();
        self.call(json!({"on":true,"lor":0,"live":true,"tt":0}))
            .await
            .ok();
    }
    async fn off(&mut self) {
        self.call(json!({"live":false})).await.ok();
        self.call(json!({"on":false,"lor":2,"tt":0})).await.ok();
    }
    async fn rgb(&self) -> Vec<u8> {
        let mut bytes = [0; 1500];
        let count = within(self.udp.recv(&mut bytes)).await.unwrap();
        assert_eq!(bytes[0], 0x41); // RGB8 DDP, push on final packet.
        assert_eq!(&bytes[2..4], &[0x0b, 1]);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0]);
        assert_eq!(
            u16::from_be_bytes([bytes[8], bytes[9]]) as usize,
            count - 10
        );
        bytes[10..count].to_vec()
    }
    async fn close(self) {
        self.shutdown.send(()).unwrap();
        within(self.server).await.unwrap();
    }
}

#[tokio::test]
async fn real_http_orders_release_before_modes_and_ddp_uses_the_existing_renderer() {
    let (mut device, network) = Device::new().await;
    let output = WledOutput::new(INSTANCE);
    let (shutdown, stopped) = oneshot::channel();
    let owner = tokio::spawn(owner::run(output.owner, network, async {
        let _ = stopped.await;
    }));
    output.control.configure(settings()).await.unwrap();
    let control = output.control.clone();
    let hold = tokio::spawn(async move { control.hold(epoch(0), red()).await });
    device.realtime().await;
    assert_eq!(device.rgb().await, [255, 0, 0]);
    assert_eq!(within(hold).await.unwrap().unwrap(), OutputApplied::Sent);
    let state = output.control.observe().await.unwrap().initial.state;
    assert!(state.held);
    assert_eq!(state.confirmation, Some(OutputApplied::Sent));
    let control = output.control.clone();
    let native =
        tokio::spawn(async move { control.finish(session(1), IdleOutputMode::Native).await });
    device.call(json!({"live":false})).await.ok();
    device.call(json!({"on":true,"lor":2,"tt":0})).await.ok();
    assert_eq!(within(native).await.unwrap().unwrap(), OutputApplied::Sent);
    shutdown.send(()).unwrap();
    device.off().await;
    assert_eq!(within(owner).await.unwrap().unwrap(), OwnerStopped);
    device.close().await;
}

#[tokio::test]
async fn refused_connection_does_not_poison_cleanup() {
    // Reserve a port without listening so the connection fails before any HTTP write.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let mut network = transport::Network::local(socket.local_addr().unwrap().port(), 4048);
    network.configure(&settings()).unwrap();
    let error = within(network.mode(Mode::Off)).await.err().unwrap();
    assert!(!error.uncertain);
}

#[tokio::test]
async fn incomplete_and_malformed_http_replies_are_uncertain_not_success() {
    for reply in [None, Some("not JSON".into()), Some("x".repeat(4097))] {
        let (mut device, network) = Device::new().await;
        let output = WledOutput::new(INSTANCE);
        let (shutdown, stopped) = oneshot::channel();
        let owner = tokio::spawn(owner::run(output.owner, network, async {
            let _ = stopped.await;
        }));
        output.control.configure(settings()).await.unwrap();
        let control = output.control.clone();
        let hold = tokio::spawn(async move { control.hold(epoch(0), EdgeColors::default()).await });
        device.call(json!({"live":false})).await.ok();
        device
            .call(json!({"on":true,"lor":0,"live":true,"tt":0}))
            .await
            .response
            .send(reply)
            .unwrap();
        assert!(within(hold).await.unwrap().is_err());
        assert!(matches!(
            output.control.finish(session(1), IdleOutputMode::Off).await,
            Err(ControlError::CleanupPending(_))
        ));
        shutdown.send(()).unwrap();
        assert!(within(owner).await.unwrap().is_err());
        assert!(device.calls.try_recv().is_err());
        device.close().await;
    }
}

#[derive(Clone)]
struct Capture(watch::Sender<Versioned<CaptureState>>);
impl Capture {
    fn state(&self, state: CaptureState) {
        self.0.send_modify(|snapshot| {
            snapshot.cursor.sequence += 1;
            snapshot.state = state;
        });
    }
}
impl CaptureControl for Capture {
    type Settings = ();
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            supports_patterns: false,
        }
    }
    fn validate(&self, _: &()) -> Result<(), ControlError> {
        Ok(())
    }
    async fn start(&self, request: CaptureStart<()>) -> Result<CaptureStarted, ControlError> {
        self.state(CaptureState::Available(StreamEpoch {
            session: request.session,
            generation: 1,
        }));
        Ok(CaptureStarted {
            session: request.session,
        })
    }
    async fn stop(&self, session: SessionId) -> Result<CaptureStopped, ControlError> {
        self.state(CaptureState::Idle);
        Ok(CaptureStopped { session })
    }
    async fn observe(&self) -> Result<Observation<CaptureState>, ControlError> {
        let mut updates = self.0.subscribe();
        let initial = updates.borrow_and_update().clone();
        Ok(Observation {
            initial,
            updates: futures::stream::unfold(updates, |mut updates| async {
                updates.changed().await.ok()?;
                let state = updates.borrow_and_update().clone();
                Some((Ok(Update::Snapshot(state)), updates))
            })
            .boxed(),
        })
    }
}
struct Frames(mpsc::Receiver<CapturedFrame>);
impl FrameSource for Frames {
    async fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ControlError> {
        Ok(self.0.recv().await)
    }
}
type AppSettings = Settings<(), WledSettings>;
struct Repository(AppSettings);
impl SettingsRepository<AppSettings> for Repository {
    async fn load(&self) -> Result<Option<AppSettings>, ControlError> {
        Ok(Some(self.0.clone()))
    }
    async fn save(&self, _: AppSettings) -> Result<(), ControlError> {
        panic!("fixture does not save")
    }
}
fn operation(request: u64) -> OperationId {
    OperationId {
        instance: INSTANCE,
        request: NonZeroU64::new(request).unwrap(),
    }
}
async fn state(
    control: &ApplicationHandle<AppSettings>,
    predicate: impl Fn(&ApplicationSnapshot<AppSettings>) -> bool,
) {
    within(async {
        let mut observation = control.observe().await.unwrap();
        if predicate(&observation.initial.state) {
            return;
        }
        while let Some(update) = observation.updates.next().await {
            if let Update::Snapshot(snapshot) = update.unwrap()
                && predicate(&snapshot.state)
            {
                return;
            }
        }
        panic!("application observation closed");
    })
    .await;
}

#[tokio::test]
async fn application_and_real_flow_drive_real_output_through_start_wait_resume_and_stop() {
    let (mut device, network) = Device::new().await;
    let output = WledOutput::new(INSTANCE);
    let late_colors = output.colors.clone();
    let (output_shutdown, output_stop) = oneshot::channel();
    let output_owner = tokio::spawn(owner::run(output.owner, network, async {
        let _ = output_stop.await;
    }));
    let capture = Capture(
        watch::channel(Versioned {
            cursor: Cursor {
                instance: INSTANCE,
                sequence: 0,
            },
            state: CaptureState::Idle,
        })
        .0,
    );
    let (feed, frames) = mpsc::channel(1);
    let config = prysm_core::Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        remove_black_bars: false,
        ..Default::default()
    };
    let flow = ProcessingFlow::connect(
        Frames(frames),
        PrysmProcessor::new(&config),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    let (flow_shutdown, flow_stop) = oneshot::channel();
    let flow_owner = tokio::spawn(flow.owner.run(INSTANCE, async {
        let _ = flow_stop.await;
    }));
    let app = Application::new(
        capture.clone(),
        flow.control,
        output.control,
        Repository(Settings {
            capture: Some(()),
            output: Some(settings()),
            processing: config,
            profiles: vec![],
            active_profile: None,
        }),
        V1Policy,
    );
    let (app_shutdown, app_stop) = oneshot::channel();
    let app_owner = tokio::spawn(app.owner.run(INSTANCE, async {
        let _ = app_stop.await;
    }));
    app.control
        .submit(Request {
            id: operation(1),
            command: ApplicationCommand::SetTarget(LightingTarget::Processed),
        })
        .await
        .unwrap();
    device.realtime().await;
    state(&app.control, |snapshot| {
        matches!(snapshot.session, SessionState::Running(_))
    })
    .await;
    feed.send(CapturedFrame {
        stamp: FrameStamp {
            epoch: epoch(1),
            sequence: 1,
        },
        frame: Frame::fill(255, 8, 4, PixelFormat::RGB24).unwrap(),
    })
    .await
    .unwrap();
    within(async { while device.rgb().await != [255, 255, 255] {} }).await;
    assert!(matches!(
        app.control.operation(operation(1)).await.unwrap(),
        OperationStatus::Succeeded
    ));

    capture.state(CaptureState::Waiting {
        epoch: epoch(2),
        reason: "no signal".into(),
    });
    state(&app.control, |snapshot| {
        snapshot
            .observed_output
            .as_ref()
            .is_some_and(|output| output.held)
    })
    .await;
    assert_eq!(device.rgb().await, [0, 0, 0]);
    let mut waiting_frame = frame(3, 1);
    waiting_frame.colors = red();
    assert_eq!(
        late_colors.offer(waiting_frame).unwrap(),
        OfferResult::Queued
    );
    capture.state(CaptureState::Available(epoch(3)));
    assert_eq!(device.rgb().await, [255, 0, 0]);
    state(&app.control, |snapshot| {
        snapshot
            .observed_output
            .as_ref()
            .is_some_and(|output| output.admitted == Some(epoch(3)))
    })
    .await;

    app.control
        .submit(Request {
            id: operation(2),
            command: ApplicationCommand::SetTarget(LightingTarget::Off),
        })
        .await
        .unwrap();
    device.off().await;
    state(&app.control, |snapshot| {
        matches!(snapshot.session, SessionState::Idle)
    })
    .await;
    assert!(matches!(
        app.control.operation(operation(2)).await.unwrap(),
        OperationStatus::Succeeded
    ));
    assert_eq!(
        late_colors.offer(frame(3, 2)).unwrap(),
        OfferResult::DiscardedStale
    );
    app_shutdown.send(()).unwrap();
    within(app_owner).await.unwrap().unwrap();
    flow_shutdown.send(()).unwrap();
    within(flow_owner).await.unwrap().unwrap();
    output_shutdown.send(()).unwrap();
    within(output_owner).await.unwrap().unwrap();
    device.close().await;
}
