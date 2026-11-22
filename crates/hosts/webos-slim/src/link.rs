//! The forwarder's side of the supervisor link, and the uplink sink that
//! keeps reconnecting to the receiver, follows its request and tells the
//! link whether frames flow.
use futures::future::BoxFuture;
use prysm_capture_remote::{
    Close, RemoteSender, RemoteSettings, SendError, StreamEncoding, WireError,
};
use prysm_capture_webos::VideoSignal;
use prysm_core::Frame;
use prysm_ports::{Error, FrameSink, Result, chain};
use prysm_runtime::{Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{
    Command, Event, FailureKind, HEARTBEAT, SlimEncoding, SlimStatus, WorkerFailure, WorkerStatus,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::time::{Instant as TokioInstant, sleep_until};

/// How often frame flow is sampled; `capturing` means frames in the last window.
const SAMPLE: Duration = Duration::from_secs(1);

/// How long the uplink waits between connection attempts while the receiver
/// is unavailable (its host is not lighting, or unreachable).
pub const RECONNECT: Duration = Duration::from_secs(1);

/// How long the uplink waits between connection attempts while Prysm speaks
/// another capture stream version. That lasts until one side is updated,
/// so it is not retried every second; an update is still picked up within
/// this time without restarting the TV app.
pub const VERSION_RECONNECT: Duration = Duration::from_secs(10);

/// What the uplink tells the capture and the link.
#[derive(Debug, Default)]
pub struct Flow {
    sent: AtomicU64,
    report: Mutex<Report>,
}

#[derive(Debug, Default)]
struct Report {
    /// The receiver's latest request, kept while it is away: the capture
    /// follows it.
    requested: Option<RemoteSettings>,
    uplink: UplinkState,
}

/// Where the uplink stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum UplinkState {
    /// No connection attempt yet.
    #[default]
    Starting,
    /// Connected; frames go out in this encoding.
    Sending(SlimEncoding),
    /// The receiver stopped the stream (its lighting is off, or its capture
    /// settings changed): the capture is released until a receiver takes a
    /// connection again.
    Stopped,
    /// The receiver is unavailable, and why.
    Waiting(String),
    /// Prysm speaks another capture stream version, and which.
    VersionDiffers(String),
    /// The requested encoder failed, and why; that request is refused until
    /// it changes.
    EncoderFailed(String),
    /// Another sender replaced this one at the receiver, and how it was
    /// told: the uplink no longer connects and the capture is released
    /// until the worker restarts, so two TVs never take the stream from
    /// each other in turn.
    Replaced(String),
}

impl Flow {
    fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// What to capture for: the receiver's latest request, kept while it is
    /// away; `None` before the first connection, while the receiver has
    /// stopped the stream, while it speaks another version and once another
    /// sender replaced this one.
    pub fn wanted(&self) -> Option<RemoteSettings> {
        let report = self.lock();
        match report.uplink {
            UplinkState::Stopped | UplinkState::VersionDiffers(_) | UplinkState::Replaced(_) => {
                None
            }
            _ => report.requested,
        }
    }

    /// The status fields the uplink owns, with `fps` frames sent per second
    /// reported while sending. A stopped stream is `paused`, not a failure.
    fn status(&self, fps: u16) -> SlimStatus {
        let (fps, encoding, paused, failure) = match &self.lock().uplink {
            UplinkState::Starting => (None, None, false, None),
            UplinkState::Sending(encoding) => (Some(fps), Some(*encoding), false, None),
            UplinkState::Stopped => (None, None, true, None),
            UplinkState::Waiting(why) => (
                None,
                None,
                false,
                Some(WorkerFailure::new(FailureKind::Receiver, why.clone())),
            ),
            UplinkState::VersionDiffers(why) => (
                None,
                None,
                false,
                Some(WorkerFailure::new(FailureKind::Version, why.clone())),
            ),
            UplinkState::EncoderFailed(why) => (
                None,
                None,
                false,
                Some(WorkerFailure::new(FailureKind::Encoder, why.clone())),
            ),
            UplinkState::Replaced(why) => (
                None,
                None,
                false,
                Some(WorkerFailure::new(FailureKind::Replaced, why.clone())),
            ),
        };
        SlimStatus {
            fps,
            failure,
            encoding,
            paused,
            ..SlimStatus::default()
        }
    }

    fn lock(&self) -> MutexGuard<'_, Report> {
        self.report.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The Luna name of a requested encoding.
#[must_use]
pub fn slim_encoding(encoding: StreamEncoding) -> SlimEncoding {
    match encoding {
        StreamEncoding::H264Hardware => SlimEncoding::H264Hardware,
        StreamEncoding::H264Software => SlimEncoding::H264Software,
        StreamEncoding::Raw => SlimEncoding::Raw,
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
/// came from the Prysm before its update), and retried only every
/// [`VERSION_RECONNECT`]: it lasts until one side is updated.
///
/// A refusal ([`SendError::Refused`]: the requested encoder cannot start or
/// failed) is reported as an encoder failure, and every later connection
/// with the same request is refused with that reason (the receiver shows
/// it) until the receiver asks for something else. There is no switch to
/// another encoder.
pub struct Uplink<S, C> {
    connect: C,
    sink: Option<S>,
    retry_at: Option<Instant>,
    retry: Duration,
    version_retry: Duration,
    flow: Arc<Flow>,
    /// The request whose encoder failed, and why.
    failed: Option<(RemoteSettings, String)>,
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> Uplink<S, C> {
    pub fn new(connect: C) -> (Self, Arc<Flow>) {
        let flow = Arc::new(Flow::default());
        let uplink = Self {
            connect,
            sink: None,
            retry_at: None,
            retry: RECONNECT,
            version_retry: VERSION_RECONNECT,
            flow: flow.clone(),
            failed: None,
        };
        (uplink, flow)
    }

    #[cfg(test)]
    pub fn retry_every(mut self, retry: Duration, version_retry: Duration) -> Self {
        self.retry = retry;
        self.version_retry = version_retry;
        self
    }

    /// Connect unless an attempt was made within [`RECONNECT`], or within
    /// [`VERSION_RECONNECT`] after Prysm answered with another version;
    /// never once replaced.
    fn reconnect(&mut self) {
        if self.retry_at.is_some_and(|at| Instant::now() < at)
            || matches!(self.flow.lock().uplink, UplinkState::Replaced(_))
        {
            return;
        }
        self.retry_at = Some(Instant::now() + self.retry);
        let mut sink = match (self.connect)() {
            Ok(sink) => sink,
            Err(error) => {
                let versions_differ = is_version_mismatch(&error);
                if versions_differ {
                    self.retry_at = Some(Instant::now() + self.version_retry);
                }
                let why = format!("waiting for the receiver: {}", chain(&error));
                let mut report = self.flow.lock();
                if versions_differ {
                    // Prysm was updated: what it said before no longer holds.
                    report.uplink = UplinkState::VersionDiffers(why);
                } else if !matches!(
                    report.uplink,
                    UplinkState::Stopped | UplinkState::EncoderFailed(_)
                ) {
                    // A stopped receiver or a failed encoder is still the news.
                    report.uplink = UplinkState::Waiting(why);
                }
                return;
            }
        };
        let request = sink.request();
        if let Some((failed, reason)) = &self.failed {
            if *failed == request {
                sink.refuse_encoding(reason);
                // The failure is the news again, whatever was shown meanwhile.
                self.flow.lock().uplink = UplinkState::EncoderFailed(reason.clone());
                return;
            }
            self.failed = None;
        }
        *self.flow.lock() = Report {
            requested: Some(request),
            uplink: UplinkState::Sending(slim_encoding(request.encoding)),
        };
        self.sink = Some(sink);
    }

    /// The connection ended, and its socket closes: remember an encoder
    /// failure, stop as the receiver said, or wait for it.
    fn lost(&mut self, error: SendError) {
        let Some(sink) = self.sink.take() else {
            return;
        };
        self.flow.lock().uplink = match error {
            SendError::Refused(error) => {
                let reason = chain(&error);
                self.failed = Some((sink.request(), reason.clone()));
                UplinkState::EncoderFailed(reason)
            }
            SendError::Connection(error) => match wire_cause(&error) {
                Some(WireError::Closed(Some(Close::Stopped))) => UplinkState::Stopped,
                Some(WireError::Closed(Some(Close::Replaced))) => {
                    UplinkState::Replaced(chain(&error))
                }
                _ => UplinkState::Waiting(format!("waiting for the receiver: {}", chain(&error))),
            },
        };
    }
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> FrameSink for Uplink<S, C> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            self.reconnect();
            return Ok(());
        };
        match sink.deliver(frame) {
            Ok(()) => {
                self.flow.sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => self.lost(error),
        }
        Ok(())
    }

    /// Reconnect, or read the receiver's messages while the capture
    /// delivers nothing.
    fn tick(&mut self) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            self.reconnect();
            return Ok(());
        };
        if let Err(error) = sink.tick() {
            self.lost(SendError::Connection(error));
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

/// Whether `error` is Prysm answering with another capture stream version.
fn is_version_mismatch(error: &Error) -> bool {
    matches!(wire_cause(error), Some(WireError::Version { .. }))
}

/// `Surface<()>`: video commands into the capture, frame flow out as status:
/// `capturing` while frames are delivered, `fps` (frames sent per second)
/// while sending, `encoding` while connected,
/// `paused` while the receiver has stopped the stream, `failure` naming the
/// encoder failure, the sender that replaced this one, Prysm's other
/// capture stream version, or else the wait while the receiver is
/// unavailable.
/// `Lighting` is protocol misuse here and stops the worker as failed.
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
                    Some(Command::Lighting { .. }) => {
                        stop.request(Stop::Failed("`lighting` is not a slim worker command".into()));
                        return Ok(());
                    }
                },
                () = sleep_until(sample) => {
                    sample = TokioInstant::now() + self.sample;
                    let now = self.flow.sent();
                    let next = SlimStatus {
                        capturing: now != counted,
                        ..self.flow.status(rate(now - counted, self.sample))
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
