//! The forwarder's side of the supervisor link, and the uplink sink that
//! keeps reconnecting to the receiver, follows its request and tells the
//! link whether frames flow.
use classify::Cause;
use futures::future::BoxFuture;
use prysm_capture_remote::{RemoteSender, RemoteSettings, SendError};
use prysm_capture_webos::VideoSignal;
use prysm_core::Frame;
use prysm_ports::{Error, Escalation, FrameSink, Readiness, Result, chain};
use prysm_runtime::{Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{Command, Event, HEARTBEAT, SlimLink, SlimStatus, WorkerStatus};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::time::{Instant as TokioInstant, sleep_until};

mod classify;

/// How often frame flow is sampled; `capturing` means frames in the last window.
const SAMPLE: Duration = Duration::from_secs(1);

/// Time between a completed attempt and the next one. The receiver can
/// return without an announcement. Each attempt is bounded by
/// [`crate::resolve::RESOLVE_WAIT`] and [`prysm_capture_remote::CONNECT_DEADLINE`].
pub const RECONNECT: Duration = Duration::from_secs(1);

/// What the uplink tells the capture and the link.
#[derive(Debug, Default)]
pub struct Flow {
    sent: AtomicU64,
    /// The open capture's last pull said the TV refuses its video; `false`
    /// while no capture is open.
    refused: AtomicBool,
    report: Mutex<Report>,
}

/// The receiver request and status shared by capture and the supervisor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Report {
    uplink: UplinkState,
    /// The request whose encoder failed, and why: refused until the
    /// receiver asks for another, also after other states came between.
    /// The receiver may not hear a refusal and connect again with the same
    /// request; without this the encoder would reopen at every connection.
    refused: Option<(RemoteSettings, String)>,
}

/// Where the uplink stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum UplinkState {
    /// No connection attempt yet.
    #[default]
    Starting,
    /// Connected; frames go out in this encoding. The only state with an
    /// open connection.
    Sending(RemoteSettings),
    /// The receiver stopped the stream (its lighting is off, or its capture
    /// settings changed): the capture is released until a receiver takes a
    /// connection again.
    Stopped,
    /// The receiver is unavailable, and why.
    Waiting(String, Option<RemoteSettings>),
    /// Prysm speaks another capture stream version, and which.
    VersionDiffers(String),
    /// The requested encoder failed, and why: that request is refused until
    /// it changes, and the capture is released meanwhile.
    EncoderFailed(String),
    /// The receiver's source failed with this stream, and how it said so:
    /// the capture is released until a receiver takes a connection again.
    ReceiverFailed(String),
}

/// Why a connection ended or could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Disconnected {
    /// The receiver is unavailable, and why: its host is not lighting, or
    /// it is unreachable.
    Absent(String),
    /// The receiver stopped the stream.
    Stopped,
    /// The receiver's source failed with this stream, and how it said so.
    Failed(String),
    /// Prysm speaks another capture stream version, and which.
    OtherVersion(String),
    /// This end refused the request (its encoder failed), and why.
    Refused {
        request: RemoteSettings,
        reason: String,
    },
}

impl Report {
    /// Keep a new connection unless its encoder already failed for this request.
    fn connected(&mut self, request: RemoteSettings) -> Result<(), String> {
        if let Some((failed, reason)) = &self.refused
            && *failed == request
        {
            self.uplink = UplinkState::EncoderFailed(reason.clone());
            return Err(reason.clone());
        }
        self.refused = None;
        self.uplink = UplinkState::Sending(request);
        Ok(())
    }

    fn disconnected(&mut self, why: Disconnected) {
        use UplinkState::{
            EncoderFailed, ReceiverFailed, Sending, Starting, Stopped, VersionDiffers, Waiting,
        };
        self.uplink = match why {
            // A retry that finds nobody must preserve a more specific diagnosis.
            Disconnected::Absent(why) => match self.uplink {
                Starting | Sending(_) | Waiting(..) => Waiting(why, self.wanted()),
                Stopped | EncoderFailed(_) | ReceiverFailed(_) | VersionDiffers(_) => return,
            },
            Disconnected::OtherVersion(why) => VersionDiffers(why),
            Disconnected::Refused { request, reason } => {
                self.refused = Some((request, reason.clone()));
                EncoderFailed(reason)
            }
            Disconnected::Stopped => Stopped,
            Disconnected::Failed(why) => ReceiverFailed(why),
        };
    }

    /// See [`Flow::wanted`].
    fn wanted(&self) -> Option<RemoteSettings> {
        match self.uplink {
            UplinkState::Sending(request) => Some(request),
            UplinkState::Waiting(_, previous) => previous,
            _ => None,
        }
    }

    /// The link as the status reports it, `fps` frames sent per second
    /// while sending, refused while sending and the capture says the TV
    /// refuses its video. A stopped stream is paused, not a failure.
    fn link(&self, fps: u16, refused: bool) -> SlimLink {
        match &self.uplink {
            UplinkState::Starting => SlimLink::Starting {},
            UplinkState::Sending(_) if refused => SlimLink::Refused {},
            UplinkState::Sending(request) => SlimLink::Sending {
                encoding: request.encoding,
                fps,
            },
            UplinkState::Stopped => SlimLink::Paused {},
            UplinkState::Waiting(why, _) => SlimLink::Waiting {
                message: why.clone(),
            },
            UplinkState::VersionDiffers(why) => SlimLink::VersionDiffers {
                message: why.clone(),
            },
            UplinkState::EncoderFailed(why) => SlimLink::EncoderFailed {
                message: why.clone(),
            },
            UplinkState::ReceiverFailed(why) => SlimLink::ReceiverFailed {
                message: why.clone(),
            },
        }
    }
}

impl Flow {
    fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Whether the open capture's last pull said the TV refuses its video
    /// (`false` while none is open).
    pub fn refused(&self, refused: bool) {
        self.refused.store(refused, Ordering::Relaxed);
    }

    /// What to capture for: the receiver's latest request, kept while it is
    /// away; `None` before the first connection, while the receiver has
    /// stopped the stream, while its request is refused (the encoder
    /// failed) and while it speaks another version. Connection attempts go
    /// on meanwhile (each idle capture wait ticks the uplink), so a new
    /// request still gets through.
    pub fn wanted(&self) -> Option<RemoteSettings> {
        self.lock().wanted()
    }

    // Take the request by value so upstream code runs before the report lock is held.
    fn connected(&self, request: RemoteSettings) -> Result<(), String> {
        self.lock().connected(request)
    }

    fn disconnected(&self, why: Disconnected) {
        self.lock().disconnected(why);
    }

    fn sending(&self) -> bool {
        matches!(self.lock().uplink, UplinkState::Sending(_))
    }

    fn link(&self, fps: u16) -> SlimLink {
        self.lock().link(fps, self.refused.load(Ordering::Relaxed))
    }

    fn lock(&self) -> MutexGuard<'_, Report> {
        self.report.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One connection to the receiver, with the receiver's request.
pub trait Upstream: Send {
    fn request(&self) -> RemoteSettings;
    /// Send one frame.
    ///
    /// # Errors
    /// This end refused the request (its encoder failed), or the connection
    /// failed or was closed.
    fn deliver(&mut self, frame: &Frame) -> std::result::Result<(), SendError>;
    /// No frame: read the receiver's messages, and so its close.
    ///
    /// # Errors
    /// The connection failed or was closed.
    fn tick(&mut self) -> Result<()>;
    /// Tell the receiver its requested encoding cannot be produced (the
    /// encoder failed), and end the stream.
    fn refuse_encoding(&mut self, reason: &str);
}

impl Upstream for RemoteSender {
    fn request(&self) -> RemoteSettings {
        RemoteSender::request(self)
    }
    fn deliver(&mut self, frame: &Frame) -> std::result::Result<(), SendError> {
        RemoteSender::deliver(self, frame)
    }
    fn tick(&mut self) -> Result<()> {
        FrameSink::tick(self)
    }
    fn refuse_encoding(&mut self, reason: &str) {
        RemoteSender::refuse_encoding(self, reason);
    }
}

/// Forwards frames to the connected receiver. On a frame or an idle tick it
/// can reconnect after [`RECONNECT`]. The frame that opens a connection is
/// dropped because it predates the receiver's request.
///
/// [`classify::heard`] decides which failures end the worker. The old socket
/// is dropped before reconnecting so a replacement close can only come from
/// another sender. [`Flow::wanted`] tells capture whether to keep running;
/// idle ticks still read the receiver's close while capture delivers nothing.
pub struct Uplink<S, C> {
    connect: C,
    /// The open connection; there is one exactly while sending.
    sink: Option<S>,
    /// When the last connection attempt returned.
    returned: Option<Instant>,
    retry: Duration,
    flow: Arc<Flow>,
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> Uplink<S, C> {
    pub fn new(connect: C) -> (Self, Arc<Flow>) {
        let flow = Arc::new(Flow::default());
        let uplink = Self {
            connect,
            sink: None,
            returned: None,
            retry: RECONNECT,
            flow: flow.clone(),
        };
        (uplink, flow)
    }

    #[cfg(test)]
    pub fn retry_every(mut self, retry: Duration) -> Self {
        self.retry = retry;
        self
    }

    /// Whether to attempt a connection at `now`: [`RECONNECT`] after the
    /// last attempt returned.
    fn due(&self, now: Instant) -> bool {
        self.returned
            .is_none_or(|at| now.saturating_duration_since(at) >= self.retry)
    }

    /// # Errors
    /// The attempt failed where waiting cannot help.
    fn reconnect(&mut self, now: Instant) -> Result<()> {
        if !self.due(now) {
            return Ok(());
        }
        let connected = (self.connect)();
        self.returned = Some(Instant::now());
        match connected {
            Ok(mut sink) => {
                let admission = self.flow.connected(sink.request());
                match admission {
                    Ok(()) => self.sink = Some(sink),
                    Err(reason) => sink.refuse_encoding(&reason),
                }
            }
            Err(error) => match classify::heard(Cause::Attempt(&error)) {
                Ok(why) => self.flow.disconnected(why),
                Err(escalation) => return Err(ended(error, escalation)),
            },
        }
        self.check();
        Ok(())
    }

    /// The connection ended, and its socket closes.
    ///
    /// # Errors
    /// It ended where waiting cannot help.
    fn lost(&mut self, error: SendError) -> Result<()> {
        if let Some(sink) = self.sink.take() {
            match classify::heard(Cause::Lost(sink.request(), &error)) {
                Ok(why) => self.flow.disconnected(why),
                Err(escalation) => return Err(ended(error.into(), escalation)),
            };
        }
        self.check();
        Ok(())
    }

    /// A connection is open exactly while sending.
    fn check(&self) {
        debug_assert_eq!(self.sink.is_some(), self.flow.sending());
    }
}

/// The uplink's failure: `error`, escalating as [`classify::heard`] says.
fn ended(error: Error, escalation: Escalation) -> Error {
    Error::adapter_escalating("the TV stopped streaming to Prysm", error, escalation)
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> FrameSink for Uplink<S, C> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            return self.reconnect(Instant::now());
        };
        match sink.deliver(frame) {
            Ok(()) => {
                self.flow.sent.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(error) => self.lost(error),
        }
    }

    /// Reconnect, or read the receiver's messages while the capture
    /// delivers nothing.
    fn tick(&mut self) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            return self.reconnect(Instant::now());
        };
        match sink.tick() {
            Ok(()) => Ok(()),
            Err(error) => self.lost(SendError::Connection(error)),
        }
    }
}

/// `Surface<()>`: video commands into the capture, frame flow out as status:
/// `capturing` while frames are delivered, and the uplink's `link`: sending
/// (with its encoding and frames sent per second), paused while the
/// receiver has stopped the stream, waiting while it is unavailable, or
/// failed with the encoder failure, Prysm's other capture stream version or
/// the receiver's failure.
/// `Target` is protocol misuse here and stops the worker as failed.
pub struct ForwarderLink {
    link: Link,
    video: VideoSignal,
    flow: Arc<Flow>,
    sample: Duration,
    heartbeat: Duration,
}

impl ForwarderLink {
    pub fn new(link: Link, video: VideoSignal, flow: Arc<Flow>) -> Self {
        Self {
            link,
            video,
            flow,
            sample: SAMPLE,
            heartbeat: HEARTBEAT,
        }
    }

    #[cfg(test)]
    pub fn with_timing(mut self, sample: Duration, heartbeat: Duration) -> Self {
        self.sample = sample;
        self.heartbeat = heartbeat;
        self
    }

    async fn serve(self, shutdown: Shutdown) -> Result<()> {
        let Link {
            mut commands,
            events,
            stop,
        } = self.link;
        let mut counted = self.flow.sent();
        let mut status = SlimStatus::default();
        events.send(&report(&status));
        let mut beat = TokioInstant::now() + self.heartbeat;
        let mut sample = TokioInstant::now() + self.sample;
        let mut initialized = false;
        loop {
            tokio::select! {
                () = shutdown.requested() => return Ok(()),
                () = self.video.initialized(), if !initialized => {
                    events.send(&Event::Initialized {});
                    initialized = true;
                }
                command = commands.next() => match command {
                    None => return Ok(()),
                    Some(Command::Video { video }) => self.video.set(video),
                    Some(Command::ResetVideo {}) => self.video.reset(),
                    Some(Command::ObserveVideo { available }) => self.video.observe(available),
                    Some(Command::Target { .. } | Command::Resume { .. } | Command::RecoverScreenSync { .. }) => {
                        stop.request(Stop::Failed("`target` is not a slim worker command".into()));
                        return Ok(());
                    }
                },
                () = sleep_until(sample) => {
                    sample = TokioInstant::now() + self.sample;
                    let now = self.flow.sent();
                    let next = SlimStatus {
                        capturing: now != counted,
                        link: self.flow.wanted()
                            .and_then(|request| match self.video.capture_readiness(&crate::capture::settings(&request)) {
                                Readiness::Waiting(error) => Some(SlimLink::CaptureWaiting { message: chain(&error) }),
                                Readiness::Ready | Readiness::Blocked(_) => None,
                            })
                            .unwrap_or_else(|| self.flow.link(rate(now - counted, self.sample))),
                    };
                    counted = now;
                    if next != status {
                        status = next;
                        events.send(&report(&status));
                        beat = TokioInstant::now() + self.heartbeat;
                    }
                }
                () = sleep_until(beat) => {
                    events.send(&report(&status));
                    beat = TokioInstant::now() + self.heartbeat;
                }
            }
        }
    }
}

fn report(status: &SlimStatus) -> Event {
    Event::Status(WorkerStatus::Slim(status.clone()))
}

/// `frames` over `window`, in whole frames per second.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a frame count per second, rounded and clamped to the u16 range"
)]
fn rate(frames: u64, window: Duration) -> u16 {
    (frames as f64 / window.as_secs_f64())
        .round()
        .clamp(0.0, f64::from(u16::MAX)) as u16
}

impl Surface<()> for ForwarderLink {
    fn run(self: Box<Self>, (): (), shutdown: Shutdown) -> BoxFuture<'static, Result<()>> {
        Box::pin(self.serve(shutdown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use UplinkState::{
        EncoderFailed, ReceiverFailed, Sending, Starting, Stopped, VersionDiffers, Waiting,
    };
    use prysm_ports::StreamEncoding;

    /// The request before each event.
    const OLD: RemoteSettings = RemoteSettings {
        width: 320,
        height: 180,
        fps: 30,
        include_ui: true,
        encoding: StreamEncoding::Raw,
    };
    /// A request the receiver changed to.
    const NEW: RemoteSettings = RemoteSettings {
        encoding: StreamEncoding::H264Software,
        ..OLD
    };
    /// The request whose encoder failed before each event.
    const FAILED: RemoteSettings = RemoteSettings {
        encoding: StreamEncoding::H264Hardware,
        ..OLD
    };

    fn text(why: &str) -> String {
        why.to_owned()
    }

    /// `uplink` after a connection with [`OLD`] and a refusal of [`FAILED`];
    /// [`Starting`] before any connection.
    fn from(uplink: UplinkState) -> Report {
        if uplink == Starting {
            return Report::default();
        }
        Report {
            uplink,
            refused: Some((FAILED, text("busy"))),
        }
    }

    /// Every state x every event: the next state, what happens to the
    /// connection, and what the capture follows. A lost connection can only
    /// come while sending (no other state holds one), but the rule is the
    /// same from any state; an absent receiver is heard alike after an
    /// attempt and after a lost connection.
    #[test]
    fn every_uplink_state_takes_every_event() {
        for (state, after_absent, wanted) in [
            (Starting, Waiting(text("w"), None), None),
            (Sending(OLD), Waiting(text("w"), Some(OLD)), Some(OLD)),
            (
                Waiting(text("w0"), Some(OLD)),
                Waiting(text("w"), Some(OLD)),
                Some(OLD),
            ),
            (Stopped, Stopped, None),
            (VersionDiffers(text("v0")), VersionDiffers(text("v0")), None),
            (EncoderFailed(text("e0")), EncoderFailed(text("e0")), None),
            (ReceiverFailed(text("f0")), ReceiverFailed(text("f0")), None),
        ] {
            let before = from(state);
            for request in [NEW, FAILED] {
                let mut next = before.clone();
                let admission = next.connected(request);
                if request == FAILED && before.refused.is_some() {
                    assert_eq!(admission, Err(text("busy")));
                    assert_eq!(next.uplink, EncoderFailed(text("busy")));
                    assert_eq!(next.wanted(), None);
                    assert_eq!(next.refused, before.refused);
                } else {
                    assert_eq!(admission, Ok(()));
                    assert_eq!(next.uplink, Sending(request));
                    assert_eq!(next.wanted(), Some(request));
                    assert_eq!(next.refused, None);
                }
            }

            for (reason, uplink, wanted) in [
                (Disconnected::Absent(text("w")), after_absent, wanted),
                (
                    Disconnected::OtherVersion(text("v")),
                    VersionDiffers(text("v")),
                    None,
                ),
                (
                    Disconnected::Refused {
                        request: OLD,
                        reason: text("e"),
                    },
                    EncoderFailed(text("e")),
                    None,
                ),
                (Disconnected::Stopped, Stopped, None),
                (
                    Disconnected::Failed(text("f")),
                    ReceiverFailed(text("f")),
                    None,
                ),
            ] {
                let shown = format!("{:?} x {reason:?}", before.uplink);
                let refused = match &reason {
                    Disconnected::Refused { request, reason } => Some((*request, reason.clone())),
                    _ => before.refused.clone(),
                };
                let mut next = before.clone();
                next.disconnected(reason);
                assert_eq!(next.uplink, uplink, "{shown}");
                assert_eq!(next.wanted(), wanted, "{shown}");
                assert_eq!(next.refused, refused, "{shown}");
            }
        }
    }

    /// What the status says in each state.
    #[test]
    fn each_state_is_reported_as_its_link() {
        for (uplink, link) in [
            (Starting, SlimLink::Starting {}),
            (
                Sending(OLD),
                SlimLink::Sending {
                    encoding: StreamEncoding::Raw,
                    fps: 7,
                },
            ),
            (Stopped, SlimLink::Paused {}),
            (
                Waiting(text("w"), Some(OLD)),
                SlimLink::Waiting { message: text("w") },
            ),
            (
                VersionDiffers(text("v")),
                SlimLink::VersionDiffers { message: text("v") },
            ),
            (
                EncoderFailed(text("e")),
                SlimLink::EncoderFailed { message: text("e") },
            ),
            (
                ReceiverFailed(text("f")),
                SlimLink::ReceiverFailed { message: text("f") },
            ),
        ] {
            assert_eq!(from(uplink.clone()).link(7, false), link);
            let refused = match uplink {
                Sending(_) => SlimLink::Refused {},
                _ => link,
            };
            assert_eq!(from(uplink).link(7, true), refused);
        }
    }

    /// The first attempt is at once; the next one [`RECONNECT`] after the
    /// last returned, however long it took.
    #[test]
    fn attempts_count_from_the_last_return() {
        let slow = Duration::from_millis(20);
        let (mut uplink, _flow) = Uplink::new(|| -> Result<RemoteSender> {
            std::thread::sleep(slow);
            Err(Error::other("cannot connect: refused"))
        });
        let began = Instant::now();
        assert!(uplink.due(began));
        uplink.reconnect(began).unwrap();
        let returned = uplink.returned.unwrap();
        assert!(returned >= began + slow);
        assert!(!uplink.due(returned + RECONNECT - Duration::from_millis(1)));
        assert!(uplink.due(returned + RECONNECT));
    }

    #[test]
    fn asking_the_receiver_for_its_request_leaves_flow_unlocked() {
        struct Receiver(Arc<Flow>);

        impl Upstream for Receiver {
            fn request(&self) -> RemoteSettings {
                // try_lock fails immediately if reconnect called us under this lock.
                let current = self.0.report.try_lock().expect("receiver can inspect flow");
                assert_eq!(current.wanted(), None);
                NEW
            }
            fn deliver(&mut self, _: &Frame) -> Result<(), SendError> {
                unreachable!()
            }
            fn tick(&mut self) -> Result<()> {
                unreachable!()
            }
            fn refuse_encoding(&mut self, _: &str) {
                unreachable!()
            }
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let (mut uplink, flow) = Uplink::new(move || Ok(Receiver(rx.try_recv().unwrap())));
        tx.send(flow.clone()).unwrap();
        uplink.reconnect(Instant::now()).unwrap();
        assert_eq!(flow.wanted(), Some(NEW));
    }
}
