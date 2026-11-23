//! The forwarder's side of the supervisor link, and the uplink sink that
//! keeps reconnecting to the receiver, follows its request and tells the
//! link whether frames flow.
use futures::future::BoxFuture;
use prysm_capture_remote::{Close, RemoteSender, RemoteSettings, SendError, WireError};
use prysm_capture_webos::VideoSignal;
use prysm_core::Frame;
use prysm_ports::{Error, FrameSink, Result, StreamEncoding, chain};
use prysm_runtime::{Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{
    Command, Event, FailureKind, HEARTBEAT, SlimLink, SlimStatus, WorkerFailure, WorkerStatus,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::time::{Instant as TokioInstant, sleep_until};

/// How often frame flow is sampled; `capturing` means frames in the last window.
const SAMPLE: Duration = Duration::from_secs(1);

/// How long the uplink waits between connection attempts while the receiver
/// is unavailable (its host is not lighting, or unreachable) or speaks
/// another capture stream version (it refuses at once, with `426`).
pub const RECONNECT: Duration = Duration::from_secs(1);

/// What the uplink tells the capture and the link.
#[derive(Debug, Default)]
pub struct Flow {
    sent: AtomicU64,
    report: Mutex<Report>,
}

/// The uplink's state machine: every rule is in [`Report::next`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Report {
    /// The receiver's latest request, kept while it is away: the capture
    /// follows it.
    requested: Option<RemoteSettings>,
    uplink: UplinkState,
    /// The request whose encoder failed, and why: refused until the
    /// receiver asks for another.
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
    Sending(StreamEncoding),
    /// The receiver stopped the stream (its lighting is off, or its capture
    /// settings changed): the capture is released until a receiver takes a
    /// connection again.
    Stopped,
    /// The receiver is unavailable, and why.
    Waiting(String),
    /// Prysm speaks another capture stream version, and which.
    VersionDiffers(String),
    /// The requested encoder failed (why is the report's `refused`): that
    /// request is refused until it changes, and the capture is released
    /// meanwhile.
    EncoderFailed,
    /// The receiver's source failed with this stream, and how it said so:
    /// the capture is released until a receiver takes a connection again.
    ReceiverFailed(String),
    /// Another sender replaced this one at the receiver, and how it was
    /// told: the uplink no longer connects and the capture is released
    /// until the worker restarts, so two TVs never take the stream from
    /// each other in turn.
    Replaced(String),
}

/// What happened to the uplink.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UplinkEvent {
    /// A connection opened with the receiver's request.
    Connected(RemoteSettings),
    /// A connection attempt failed, and why.
    Unavailable(String),
    /// Prysm answered with another capture stream version, and which.
    OtherVersion(String),
    /// The open connection ended.
    Lost(Loss),
}

/// How the open connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Loss {
    /// This end refused the request (its encoder failed), and why.
    Refused {
        request: RemoteSettings,
        reason: String,
    },
    /// The receiver closed the stream as stopped.
    Stopped,
    /// The receiver closed the stream as replaced, and how it said so.
    Replaced(String),
    /// The receiver closed the stream as failed, and how it said so.
    Failed(String),
    /// Anything else (a crash, the network), and why.
    Dropped(String),
}

impl UplinkEvent {
    fn attempt_failed(error: &Error) -> Self {
        let why = format!("waiting for the receiver: {}", chain(error));
        if matches!(wire_cause(error), Some(WireError::Version { .. })) {
            Self::OtherVersion(why)
        } else {
            Self::Unavailable(why)
        }
    }

    fn lost(request: RemoteSettings, error: &SendError) -> Self {
        Self::Lost(match error {
            SendError::Refused(error) => Loss::Refused {
                request,
                reason: chain(error),
            },
            SendError::Connection(error) => match wire_cause(error) {
                Some(WireError::Closed(Some(Close::Stopped))) => Loss::Stopped,
                Some(WireError::Closed(Some(Close::Replaced))) => Loss::Replaced(chain(error)),
                Some(WireError::Closed(Some(Close::Failed))) => Loss::Failed(chain(error)),
                _ => Loss::Dropped(format!("waiting for the receiver: {}", chain(error))),
            },
        })
    }
}

/// What the uplink does with a connection after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    /// Keep the new connection and send on it.
    Keep,
    /// Refuse the new connection's request with this reason; it closes.
    Refuse(String),
    /// No connection stays open.
    Drop,
}

impl Report {
    /// The state after `event`, and what to do with the connection. Once
    /// replaced, nothing changes. A connection with the refused request is
    /// refused again; any other is sent on. Another version replaces
    /// whatever was shown (it came from the Prysm before its update); an
    /// unavailable receiver does not replace a stopped stream, an encoder
    /// failure, a failed receiver or another version, which are still the
    /// news.
    fn next(&self, event: UplinkEvent) -> (Self, Action) {
        use UplinkState::{
            EncoderFailed, ReceiverFailed, Replaced, Sending, Stopped, VersionDiffers, Waiting,
        };
        let shown = |uplink| Self {
            uplink,
            ..self.clone()
        };
        match (&self.uplink, event) {
            (Replaced(_), _)
            | (
                Stopped | EncoderFailed | ReceiverFailed(_) | VersionDiffers(_),
                UplinkEvent::Unavailable(_),
            ) => (self.clone(), Action::Drop),
            (_, UplinkEvent::Connected(request)) => match &self.refused {
                Some((refused, reason)) if *refused == request => {
                    (shown(EncoderFailed), Action::Refuse(reason.clone()))
                }
                _ => {
                    let next = Self {
                        requested: Some(request),
                        uplink: Sending(request.encoding),
                        refused: None,
                    };
                    (next, Action::Keep)
                }
            },
            (_, UplinkEvent::OtherVersion(why)) => (shown(VersionDiffers(why)), Action::Drop),
            (_, UplinkEvent::Unavailable(why) | UplinkEvent::Lost(Loss::Dropped(why))) => {
                (shown(Waiting(why)), Action::Drop)
            }
            (_, UplinkEvent::Lost(Loss::Refused { request, reason })) => {
                let next = Self {
                    uplink: EncoderFailed,
                    refused: Some((request, reason)),
                    ..self.clone()
                };
                (next, Action::Drop)
            }
            (_, UplinkEvent::Lost(Loss::Stopped)) => (shown(Stopped), Action::Drop),
            (_, UplinkEvent::Lost(Loss::Replaced(why))) => (shown(Replaced(why)), Action::Drop),
            (_, UplinkEvent::Lost(Loss::Failed(why))) => (shown(ReceiverFailed(why)), Action::Drop),
        }
    }

    /// See [`Flow::wanted`].
    fn wanted(&self) -> Option<RemoteSettings> {
        match self.uplink {
            UplinkState::Stopped
            | UplinkState::EncoderFailed
            | UplinkState::ReceiverFailed(_)
            | UplinkState::VersionDiffers(_)
            | UplinkState::Replaced(_) => None,
            UplinkState::Starting | UplinkState::Sending(_) | UplinkState::Waiting(_) => {
                self.requested
            }
        }
    }

    /// The link as the status reports it, `fps` frames sent per second
    /// while sending. A stopped stream is paused, not a failure.
    fn link(&self, fps: u16) -> SlimLink {
        let failed = |kind, why: &String| SlimLink::Failed {
            failure: WorkerFailure::new(kind, why.clone()),
        };
        match &self.uplink {
            UplinkState::Starting => SlimLink::Starting {},
            UplinkState::Sending(encoding) => SlimLink::Sending {
                encoding: *encoding,
                fps,
            },
            UplinkState::Stopped => SlimLink::Paused {},
            UplinkState::Waiting(why) => SlimLink::Waiting {
                failure: WorkerFailure::new(FailureKind::Receiver, why.clone()),
            },
            UplinkState::VersionDiffers(why) => failed(FailureKind::Version, why),
            UplinkState::EncoderFailed => {
                // `next` enters it only together with the refusal.
                let (_, why) = self
                    .refused
                    .as_ref()
                    .expect("an encoder failure keeps its refusal");
                failed(FailureKind::Encoder, why)
            }
            UplinkState::ReceiverFailed(why) => failed(FailureKind::ReceiverFailed, why),
            UplinkState::Replaced(why) => failed(FailureKind::Replaced, why),
        }
    }
}

impl Flow {
    fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// What to capture for: the receiver's latest request, kept while it is
    /// away; `None` before the first connection, while the receiver has
    /// stopped the stream, while its request is refused (the encoder
    /// failed), while it speaks another version and once another sender
    /// replaced this one. Connection attempts go on meanwhile (each idle
    /// capture wait ticks the uplink), so a new request still gets through.
    pub fn wanted(&self) -> Option<RemoteSettings> {
        self.lock().wanted()
    }

    fn link(&self, fps: u16) -> SlimLink {
        self.lock().link(fps)
    }

    /// Take `event` ([`Report::next`]) and say what to do with the
    /// connection.
    fn apply(&self, event: UplinkEvent) -> Action {
        let mut report = self.lock();
        let (next, action) = report.next(event);
        *report = next;
        action
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

/// A [`FrameSink`] that delivers to whatever `connect` opens and keeps
/// reconnecting; it never fails. The receiver only listens while its host is
/// lighting, so a refused connection or a dropped receiver is a normal wait:
/// frames are dropped, a new connection is tried at most every
/// [`RECONNECT`] (on the next frame or tick after it), and [`Flow`]
/// carries the reason. Each connection publishes the receiver's request,
/// which the capture follows; the frame that opens a connection is dropped,
/// since it was captured before the request was known.
///
/// A receiver that closes the stream as stopped (its lighting is off)
/// stops the capture through [`Flow::wanted`] until a receiver takes a
/// connection again; ticks keep reading its messages while nothing is
/// captured, so the close is seen. One that vanishes without saying so (a
/// crash, the network) leaves the capture running.
///
/// A receiver that closes the stream as replaced (another sender took it
/// over) stops the uplink: it releases the capture, reports a failure of
/// kind `replaced` and connects no more. That is this process's state, so it
/// lasts until the worker restarts: capture stopped and started in the TV
/// app, the TV waking from standby, a new Prysm address, or a restart after
/// a failure. The replaced connection is always the live one: the uplink
/// drops a failed connection, closing its socket, before it connects again,
/// so a TV that reconnects after a network drop never hears its own old
/// connection replaced.
///
/// A Prysm of another capture stream version ([`WireError::Version`]) is
/// reported as such, replacing a stopped stream or an encoder failure (they
/// came from the Prysm before its update), and retried like an unavailable
/// receiver: it lasts until one side is updated, also while the receiver is
/// unreachable meanwhile, and the capture stays closed.
///
/// A receiver that closes the stream as failed (its source failed with this
/// stream: a refusal, a keyframe that does not decode, no decoder) is
/// reported as a failure of kind `receiver_failed`, with the capture
/// released, until a receiver takes a connection again.
///
/// A refusal ([`SendError::Refused`]: the requested encoder cannot start or
/// failed) is reported as an encoder failure, and every later connection
/// with the same request is refused with that reason (the receiver shows
/// it) until the receiver asks for something else; the capture is released
/// meanwhile. There is no switch to another encoder.
pub struct Uplink<S, C> {
    connect: C,
    /// The open connection; there is one exactly while sending.
    sink: Option<S>,
    /// When the last connection attempt began.
    attempted: Option<Instant>,
    retry: Duration,
    flow: Arc<Flow>,
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> Uplink<S, C> {
    pub fn new(connect: C) -> (Self, Arc<Flow>) {
        let flow = Arc::new(Flow::default());
        let uplink = Self {
            connect,
            sink: None,
            attempted: None,
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
    /// last attempt, never once replaced.
    fn due(&self, now: Instant) -> bool {
        if matches!(self.flow.lock().uplink, UplinkState::Replaced(_)) {
            return false;
        }
        self.attempted
            .is_none_or(|at| now.saturating_duration_since(at) >= self.retry)
    }

    fn reconnect(&mut self, now: Instant) {
        if !self.due(now) {
            return;
        }
        self.attempted = Some(now);
        let (event, sink) = match (self.connect)() {
            Ok(sink) => (UplinkEvent::Connected(sink.request()), Some(sink)),
            Err(error) => (UplinkEvent::attempt_failed(&error), None),
        };
        match (self.flow.apply(event), sink) {
            (Action::Keep, sink) => self.sink = sink,
            (Action::Refuse(reason), Some(mut sink)) => sink.refuse_encoding(&reason),
            (Action::Refuse(_) | Action::Drop, _) => {}
        }
        self.check();
    }

    /// The connection ended, and its socket closes.
    fn lost(&mut self, error: &SendError) {
        if let Some(sink) = self.sink.take() {
            self.flow.apply(UplinkEvent::lost(sink.request(), error));
        }
        self.check();
    }

    /// A connection is open exactly while sending.
    fn check(&self) {
        debug_assert_eq!(
            self.sink.is_some(),
            matches!(self.flow.lock().uplink, UplinkState::Sending(_))
        );
    }
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> FrameSink for Uplink<S, C> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            self.reconnect(Instant::now());
            return Ok(());
        };
        match sink.deliver(frame) {
            Ok(()) => {
                self.flow.sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => self.lost(&error),
        }
        Ok(())
    }

    /// Reconnect, or read the receiver's messages while the capture
    /// delivers nothing.
    fn tick(&mut self) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            self.reconnect(Instant::now());
            return Ok(());
        };
        if let Err(error) = sink.tick() {
            self.lost(&SendError::Connection(error));
        }
        Ok(())
    }
}

/// The wire cause of an uplink error, if it has one.
fn wire_cause(error: &Error) -> Option<&WireError> {
    match error {
        Error::Adapter {
            source: Some(source),
            ..
        } => source.downcast_ref(),
        _ => None,
    }
}

/// `Surface<()>`: video commands into the capture, frame flow out as status:
/// `capturing` while frames are delivered, and the uplink's `link`: sending
/// (with its encoding and frames sent per second), paused while the
/// receiver has stopped the stream, waiting while it is unavailable, or
/// failed with the encoder failure, Prysm's other capture stream version or
/// the sender that replaced this one.
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
        loop {
            tokio::select! {
                () = shutdown.requested() => return Ok(()),
                command = commands.next() => match command {
                    None => return Ok(()),
                    Some(Command::Video { video }) => self.video.set(video),
                    Some(Command::ResetVideo {}) => self.video.reset(),
                    Some(Command::Target { .. }) => {
                        stop.request(Stop::Failed("`target` is not a slim worker command".into()));
                        return Ok(());
                    }
                },
                () = sleep_until(sample) => {
                    sample = TokioInstant::now() + self.sample;
                    let now = self.flow.sent();
                    let next = SlimStatus {
                        capturing: now != counted,
                        link: self.flow.link(rate(now - counted, self.sample)),
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
        EncoderFailed, ReceiverFailed, Replaced, Sending, Starting, Stopped, VersionDiffers,
        Waiting,
    };

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
            requested: Some(OLD),
            uplink,
            refused: Some((FAILED, text("busy"))),
        }
    }

    /// Every state x every event: the next state, what happens to the
    /// connection, and what the capture follows. A lost connection can only
    /// come while sending (no other state holds one), but the rule is the
    /// same from any state.
    #[test]
    fn every_uplink_state_takes_every_event() {
        let connected = |request| UplinkEvent::Connected(request);
        let unavailable = || UplinkEvent::Unavailable(text("w"));
        let version = || UplinkEvent::OtherVersion(text("v"));
        let refused = || {
            UplinkEvent::Lost(Loss::Refused {
                request: OLD,
                reason: text("e"),
            })
        };
        let stopped = || UplinkEvent::Lost(Loss::Stopped);
        let replaced = || UplinkEvent::Lost(Loss::Replaced(text("r")));
        let failed = || UplinkEvent::Lost(Loss::Failed(text("f")));
        let dropped = || UplinkEvent::Lost(Loss::Dropped(text("d")));
        let software = Sending(StreamEncoding::H264Software);
        let hardware = Sending(StreamEncoding::H264Hardware);
        let raw = Sending(StreamEncoding::Raw);
        let (keep, drop, refuse) = (Action::Keep, Action::Drop, Action::Refuse(text("busy")));
        #[rustfmt::skip]
        let table = [
            (Starting, connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (Starting, connected(FAILED), hardware, keep.clone(), Some(FAILED)),
            (Starting, unavailable(), Waiting(text("w")), drop.clone(), None),
            (Starting, version(), VersionDiffers(text("v")), drop.clone(), None),
            (Starting, refused(), EncoderFailed, drop.clone(), None),
            (Starting, stopped(), Stopped, drop.clone(), None),
            (Starting, replaced(), Replaced(text("r")), drop.clone(), None),
            (Starting, failed(), ReceiverFailed(text("f")), drop.clone(), None),
            (Starting, dropped(), Waiting(text("d")), drop.clone(), None),

            (raw.clone(), connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (raw.clone(), connected(FAILED), EncoderFailed, refuse.clone(), None),
            (raw.clone(), unavailable(), Waiting(text("w")), drop.clone(), Some(OLD)),
            (raw.clone(), version(), VersionDiffers(text("v")), drop.clone(), None),
            (raw.clone(), refused(), EncoderFailed, drop.clone(), None),
            (raw.clone(), stopped(), Stopped, drop.clone(), None),
            (raw.clone(), replaced(), Replaced(text("r")), drop.clone(), None),
            (raw.clone(), failed(), ReceiverFailed(text("f")), drop.clone(), None),
            (raw.clone(), dropped(), Waiting(text("d")), drop.clone(), Some(OLD)),

            (Stopped, connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (Stopped, connected(FAILED), EncoderFailed, refuse.clone(), None),
            (Stopped, unavailable(), Stopped, drop.clone(), None),
            (Stopped, version(), VersionDiffers(text("v")), drop.clone(), None),
            (Stopped, refused(), EncoderFailed, drop.clone(), None),
            (Stopped, stopped(), Stopped, drop.clone(), None),
            (Stopped, replaced(), Replaced(text("r")), drop.clone(), None),
            (Stopped, failed(), ReceiverFailed(text("f")), drop.clone(), None),
            (Stopped, dropped(), Waiting(text("d")), drop.clone(), Some(OLD)),

            (Waiting(text("w0")), connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (Waiting(text("w0")), connected(FAILED), EncoderFailed, refuse.clone(), None),
            (Waiting(text("w0")), unavailable(), Waiting(text("w")), drop.clone(), Some(OLD)),
            (Waiting(text("w0")), version(), VersionDiffers(text("v")), drop.clone(), None),
            (Waiting(text("w0")), refused(), EncoderFailed, drop.clone(), None),
            (Waiting(text("w0")), stopped(), Stopped, drop.clone(), None),
            (Waiting(text("w0")), replaced(), Replaced(text("r")), drop.clone(), None),
            (Waiting(text("w0")), failed(), ReceiverFailed(text("f")), drop.clone(), None),
            (Waiting(text("w0")), dropped(), Waiting(text("d")), drop.clone(), Some(OLD)),

            (VersionDiffers(text("v0")), connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (VersionDiffers(text("v0")), connected(FAILED), EncoderFailed, refuse.clone(), None),
            (VersionDiffers(text("v0")), unavailable(), VersionDiffers(text("v0")), drop.clone(), None),
            (VersionDiffers(text("v0")), version(), VersionDiffers(text("v")), drop.clone(), None),
            (VersionDiffers(text("v0")), refused(), EncoderFailed, drop.clone(), None),
            (VersionDiffers(text("v0")), stopped(), Stopped, drop.clone(), None),
            (VersionDiffers(text("v0")), replaced(), Replaced(text("r")), drop.clone(), None),
            (VersionDiffers(text("v0")), failed(), ReceiverFailed(text("f")), drop.clone(), None),
            (VersionDiffers(text("v0")), dropped(), Waiting(text("d")), drop.clone(), Some(OLD)),

            (EncoderFailed, connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (EncoderFailed, connected(FAILED), EncoderFailed, refuse.clone(), None),
            (EncoderFailed, unavailable(), EncoderFailed, drop.clone(), None),
            (EncoderFailed, version(), VersionDiffers(text("v")), drop.clone(), None),
            (EncoderFailed, refused(), EncoderFailed, drop.clone(), None),
            (EncoderFailed, stopped(), Stopped, drop.clone(), None),
            (EncoderFailed, replaced(), Replaced(text("r")), drop.clone(), None),
            (EncoderFailed, failed(), ReceiverFailed(text("f")), drop.clone(), None),
            (EncoderFailed, dropped(), Waiting(text("d")), drop.clone(), Some(OLD)),

            (ReceiverFailed(text("f0")), connected(NEW), software, keep, Some(NEW)),
            (ReceiverFailed(text("f0")), connected(FAILED), EncoderFailed, refuse, None),
            (ReceiverFailed(text("f0")), unavailable(), ReceiverFailed(text("f0")), drop.clone(), None),
            (ReceiverFailed(text("f0")), version(), VersionDiffers(text("v")), drop.clone(), None),
            (ReceiverFailed(text("f0")), refused(), EncoderFailed, drop.clone(), None),
            (ReceiverFailed(text("f0")), stopped(), Stopped, drop.clone(), None),
            (ReceiverFailed(text("f0")), replaced(), Replaced(text("r")), drop.clone(), None),
            (ReceiverFailed(text("f0")), failed(), ReceiverFailed(text("f")), drop.clone(), None),
            (ReceiverFailed(text("f0")), dropped(), Waiting(text("d")), drop.clone(), Some(OLD)),

            (Replaced(text("r0")), connected(NEW), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), connected(FAILED), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), unavailable(), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), version(), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), refused(), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), stopped(), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), replaced(), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), failed(), Replaced(text("r0")), drop.clone(), None),
            (Replaced(text("r0")), dropped(), Replaced(text("r0")), drop, None),
        ];
        assert_eq!(table.len(), 8 * 9);
        for (state, event, uplink, action, wanted) in table {
            let before = from(state);
            let shown = format!("{:?} x {event:?}", before.uplink);
            let (next, done) = before.next(event.clone());
            assert_eq!((&next.uplink, &done), (&uplink, &action), "{shown}");
            assert_eq!(next.wanted(), wanted, "{shown}");
            // A request is taken only from a connection that is sent on.
            let requested = match (&event, &done) {
                (UplinkEvent::Connected(request), Action::Keep) => Some(*request),
                _ => before.requested,
            };
            assert_eq!(next.requested, requested, "{shown}");
            // Exactly a kept connection is sent on.
            assert_eq!(
                done == Action::Keep,
                matches!(uplink, Sending(_)),
                "{shown}"
            );
            // A refusal is remembered; a request that is sent on clears it.
            let refused = match (&before.uplink, &event, &done) {
                (Replaced(_), _, _) => before.refused.clone(),
                (_, UplinkEvent::Lost(Loss::Refused { request, reason }), _) => {
                    Some((*request, reason.clone()))
                }
                (_, _, Action::Keep) => None,
                _ => before.refused.clone(),
            };
            assert_eq!(next.refused, refused, "{shown}");
        }
    }

    /// Each close code the receiver sends is its own loss; no code is a
    /// dropped connection.
    #[test]
    fn each_close_is_its_own_loss() {
        let closed = |close| {
            SendError::Connection(Error::adapter(
                "the stream failed",
                WireError::Closed(close),
            ))
        };
        for (close, loss) in [
            (Some(Close::Stopped), Loss::Stopped),
            (
                Some(Close::Replaced),
                Loss::Replaced(text(
                    "the stream failed: another sender replaced this one at Prysm",
                )),
            ),
            (
                Some(Close::Failed),
                Loss::Failed(text(
                    "the stream failed: Prysm stopped receiving after a failure, which it shows",
                )),
            ),
            (
                None,
                Loss::Dropped(text(
                    "waiting for the receiver: the stream failed: the peer closed the stream",
                )),
            ),
        ] {
            assert_eq!(
                UplinkEvent::lost(OLD, &closed(close)),
                UplinkEvent::Lost(loss),
                "{close:?}"
            );
        }
    }

    /// What the status says in each state.
    #[test]
    fn each_state_is_reported_as_its_link() {
        let failed = |kind, why: &str| SlimLink::Failed {
            failure: WorkerFailure::new(kind, why),
        };
        for (uplink, link) in [
            (Starting, SlimLink::Starting {}),
            (
                Sending(StreamEncoding::Raw),
                SlimLink::Sending {
                    encoding: StreamEncoding::Raw,
                    fps: 7,
                },
            ),
            (Stopped, SlimLink::Paused {}),
            (
                Waiting(text("w")),
                SlimLink::Waiting {
                    failure: WorkerFailure::new(FailureKind::Receiver, "w"),
                },
            ),
            (VersionDiffers(text("v")), failed(FailureKind::Version, "v")),
            (EncoderFailed, failed(FailureKind::Encoder, "busy")),
            (
                ReceiverFailed(text("f")),
                failed(FailureKind::ReceiverFailed, "f"),
            ),
            (Replaced(text("r")), failed(FailureKind::Replaced, "r")),
        ] {
            assert_eq!(from(uplink).link(7), link);
        }
    }

    /// A connection that is never opened.
    struct Never;

    impl Upstream for Never {
        fn request(&self) -> RemoteSettings {
            unreachable!()
        }
        fn deliver(&mut self, _frame: &Frame) -> std::result::Result<(), SendError> {
            unreachable!()
        }
        fn tick(&mut self) -> Result<()> {
            unreachable!()
        }
        fn refuse_encoding(&mut self, _reason: &str) {
            unreachable!()
        }
    }

    /// The first attempt is at once; the next one [`RECONNECT`] after the
    /// last, and none once replaced.
    #[test]
    fn attempts_follow_the_state() {
        let (mut uplink, flow) = Uplink::new(|| -> Result<Never> { unreachable!() });
        let t0 = Instant::now();
        let after = |ms| t0 + Duration::from_millis(ms);
        assert!(uplink.due(t0));
        uplink.attempted = Some(t0);
        let wait = u64::try_from(RECONNECT.as_millis()).unwrap();
        for state in [
            Starting,
            Stopped,
            Waiting(text("w")),
            EncoderFailed,
            ReceiverFailed(text("f")),
            VersionDiffers(text("v")),
        ] {
            flow.lock().uplink = state.clone();
            assert!(!uplink.due(after(wait - 1)), "{state:?}");
            assert!(uplink.due(after(wait)), "{state:?}");
        }
        flow.lock().uplink = Replaced(text("r"));
        assert!(!uplink.due(after(3_600_000)));
        uplink.attempted = None;
        assert!(!uplink.due(t0));
    }
}
