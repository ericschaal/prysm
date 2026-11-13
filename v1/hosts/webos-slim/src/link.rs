//! The forwarder's side of the supervisor link, and the uplink sink that
//! keeps reconnecting to the receiver, follows its request and tells the
//! link whether frames flow.
use futures::future::BoxFuture;
use prysm_capture_remote::{RemoteSender, SendError, StreamEncoding, StreamRequest};
use prysm_capture_webos::VideoSignal;
use prysm_core::Frame;
use prysm_ports::{FrameSink, Result, chain};
use prysm_runtime::{Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{
    Command, Event, FailureKind, HEARTBEAT, SlimEncoding, WorkerFailure, WorkerStatus,
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
    requested: Option<StreamRequest>,
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
    /// The receiver paused the stream (its lighting is off): the capture
    /// stops. The encoding while still connected. A receiver that pauses and
    /// then closes the connection, as it does when lighting stops, leaves
    /// the uplink paused until a receiver connects again.
    Paused(Option<SlimEncoding>),
    /// The receiver is unavailable, and why.
    Waiting(String),
    /// The requested encoder failed, and why; that request is refused until
    /// it changes.
    EncoderFailed(String),
}

impl UplinkState {
    /// A connected `sink`: sending, or paused by the receiver.
    fn connected(sink: &impl Upstream) -> Self {
        let encoding = slim_encoding(sink.request().encoding);
        if sink.paused() {
            Self::Paused(Some(encoding))
        } else {
            Self::Sending(encoding)
        }
    }
}

impl Flow {
    fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// What to capture for: the receiver's latest request, kept while it is
    /// away; `None` before the first connection and while it has paused
    /// the stream.
    pub fn wanted(&self) -> Option<StreamRequest> {
        let report = self.lock();
        match report.uplink {
            UplinkState::Paused(_) => None,
            _ => report.requested,
        }
    }

    /// The status fields the uplink owns, with `fps` frames sent per second
    /// reported while sending. A paused stream is not a failure.
    fn status(&self, fps: u16) -> WorkerStatus {
        let (fps, encoding, paused, failure) = match &self.lock().uplink {
            UplinkState::Starting => (None, None, false, None),
            UplinkState::Sending(encoding) => (Some(fps), Some(*encoding), false, None),
            UplinkState::Paused(encoding) => (None, *encoding, true, None),
            UplinkState::Waiting(why) => (
                None,
                None,
                false,
                Some(WorkerFailure::new(FailureKind::Receiver, why.clone())),
            ),
            UplinkState::EncoderFailed(why) => (
                None,
                None,
                false,
                Some(WorkerFailure::new(FailureKind::Encoder, why.clone())),
            ),
        };
        WorkerStatus {
            fps,
            failure,
            encoding,
            paused,
            ..WorkerStatus::default()
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

/// One connection to the receiver, with the receiver's request; the
/// receiver can pause it.
pub trait Upstream: Send {
    fn request(&self) -> StreamRequest;
    /// The receiver does not want frames; `deliver` drops them.
    fn paused(&self) -> bool;
    /// Send one frame.
    ///
    /// # Errors
    /// The requested encoder failed, or the connection did.
    fn deliver(&mut self, frame: &Frame) -> std::result::Result<(), SendError>;
    /// No frame: read the receiver's pause and resume.
    ///
    /// # Errors
    /// The connection failed.
    fn tick(&mut self) -> Result<()>;
    /// Tell the receiver its requested encoding cannot be produced (the
    /// encoder failed), and end the stream.
    fn refuse_encoding(&mut self, reason: &str);
}

impl Upstream for RemoteSender {
    fn request(&self) -> StreamRequest {
        RemoteSender::request(self)
    }
    fn paused(&self) -> bool {
        RemoteSender::paused(self)
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
/// A receiver that pauses the stream (its lighting is off) stops the
/// capture through [`Flow::wanted`]; ticks keep reading its control
/// messages, so a resume starts it again. A receiver that pauses and then
/// closes (lighting stopped) leaves the capture stopped until a receiver
/// connects again; one that vanishes without pausing (a crash, the
/// network) leaves it running.
///
/// An encoder failure ([`SendError::Encoder`]: the requested encoder cannot
/// start or failed) is reported as such, and every later connection
/// with the same request is refused with that reason (the receiver shows
/// it) until the receiver asks for something else. There is no switch to
/// another encoder.
pub struct Uplink<S, C> {
    connect: C,
    sink: Option<S>,
    retry_at: Option<Instant>,
    retry: Duration,
    flow: Arc<Flow>,
    /// The request whose encoder failed, and why.
    failed: Option<(StreamRequest, String)>,
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> Uplink<S, C> {
    pub fn new(connect: C) -> (Self, Arc<Flow>) {
        let flow = Arc::new(Flow::default());
        let uplink = Self {
            connect,
            sink: None,
            retry_at: None,
            retry: RECONNECT,
            flow: flow.clone(),
            failed: None,
        };
        (uplink, flow)
    }

    #[cfg(test)]
    pub fn retry_every(mut self, retry: Duration) -> Self {
        self.retry = retry;
        self
    }

    /// Connect unless an attempt was made within [`RECONNECT`].
    fn reconnect(&mut self) {
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        self.retry_at = Some(Instant::now() + self.retry);
        let mut sink = match (self.connect)() {
            Ok(sink) => sink,
            Err(error) => {
                let mut report = self.flow.lock();
                // A paused receiver or a failed encoder is still the news.
                if !matches!(
                    report.uplink,
                    UplinkState::Paused(_) | UplinkState::EncoderFailed(_)
                ) {
                    report.uplink = UplinkState::Waiting(format!(
                        "waiting for the receiver: {}",
                        chain(&error)
                    ));
                }
                return;
            }
        };
        let request = sink.request();
        if let Some((failed, reason)) = &self.failed {
            if *failed == request {
                sink.refuse_encoding(reason);
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

    /// The connection ended: wait for the receiver, or remember an encoder
    /// failure. A receiver that paused before closing stays paused.
    fn lost(&mut self, error: SendError) {
        let Some(sink) = self.sink.take() else {
            return;
        };
        self.flow.lock().uplink = match error {
            SendError::Encoder(error) => {
                let reason = chain(&error);
                self.failed = Some((sink.request(), reason.clone()));
                UplinkState::EncoderFailed(reason)
            }
            SendError::Connection(_) if sink.paused() => UplinkState::Paused(None),
            SendError::Connection(error) => {
                UplinkState::Waiting(format!("waiting for the receiver: {}", chain(&error)))
            }
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
                if !sink.paused() {
                    self.flow.sent.fetch_add(1, Ordering::Relaxed);
                }
                self.flow.lock().uplink = UplinkState::connected(sink);
            }
            Err(error) => self.lost(error),
        }
        Ok(())
    }

    /// Reconnect, or read the receiver's pause and resume while the
    /// capture delivers nothing (it is stopped while paused).
    fn tick(&mut self) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            self.reconnect();
            return Ok(());
        };
        match sink.tick() {
            Ok(()) => self.flow.lock().uplink = UplinkState::connected(sink),
            Err(error) => self.lost(SendError::Connection(error)),
        }
        Ok(())
    }
}

/// `Surface<()>`: video commands into the capture, frame flow out as status:
/// `capturing` while frames are delivered, `fps` (frames sent per second)
/// while sending, `encoding` while connected,
/// `paused` while the receiver paused the stream, `failure` naming the
/// encoder failure, or else the wait while the receiver is unavailable.
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
        let mut status = WorkerStatus::default();
        events.send(&Event::Status(status.clone()));
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
                    let next = WorkerStatus {
                        capturing: now != counted,
                        ..self.flow.status(rate(now - counted, self.sample))
                    };
                    counted = now;
                    if next != status {
                        status = next;
                        events.send(&Event::Status(status.clone()));
                        beat = TokioInstant::now() + self.heartbeat;
                    }
                }
                () = sleep_until(beat) => {
                    events.send(&Event::Status(status.clone()));
                    beat = TokioInstant::now() + self.heartbeat;
                }
            }
        }
    }
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
