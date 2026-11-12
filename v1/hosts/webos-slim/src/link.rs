//! The forwarder's side of the supervisor link, and the uplink sink that
//! keeps reconnecting to the receiver, follows its request and tells the
//! link whether frames flow.
use crate::video::VideoFeed;
use futures::future::BoxFuture;
use prysm_capture_remote::{RemoteSender, StreamEncoding, StreamRequest};
use prysm_core::Frame;
use prysm_h264::H264Error;
use prysm_ports::{Error, FrameSink, Result, chain};
use prysm_runtime::{Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{Command, Event, FailureKind, HEARTBEAT, SlimEncoding, WorkerStatus};
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
    /// The encoding being sent, while connected.
    encoding: Option<SlimEncoding>,
    /// The receiver paused the stream (its lighting is off): the capture
    /// stops. Kept when the receiver then closes the connection, as it does
    /// when lighting stops, until a receiver connects again.
    paused: bool,
    /// Why no frames can be delivered: the receiver is unavailable.
    waiting: Option<String>,
    /// The requested encoder failed; that request is refused until it changes.
    encoder: Option<String>,
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
        if report.paused {
            None
        } else {
            report.requested
        }
    }

    /// The status fields the uplink owns. A paused stream is not a failure.
    fn status(&self) -> WorkerStatus {
        let report = self.lock();
        let paused = report.paused && report.encoder.is_none();
        let (failure, failure_kind) = match (&report.encoder, &report.waiting) {
            (Some(failure), _) => (Some(failure.clone()), Some(FailureKind::Encoder)),
            (None, _) if paused => (None, None),
            (None, Some(waiting)) => (Some(waiting.clone()), Some(FailureKind::Receiver)),
            (None, None) => (None, None),
        };
        WorkerStatus {
            failure,
            failure_kind,
            encoding: report.encoding,
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

/// One connection to the receiver: a frame sink with the receiver's request
/// that the receiver can pause.
pub trait Upstream: FrameSink {
    fn request(&self) -> StreamRequest;
    /// The receiver does not want frames; `send` drops them.
    fn paused(&self) -> bool;
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
    fn refuse_encoding(&mut self, reason: &str) {
        RemoteSender::refuse_encoding(self, reason);
    }
}

/// A [`FrameSink`] that delivers to whatever `connect` opens and keeps
/// reconnecting; it never fails. The receiver only listens while its host is
/// lighting, so a refused connection or a dropped receiver is a normal wait:
/// frames are dropped, a new connection is tried at most every
/// [`RECONNECT`] (on the next frame or idle tick after it), and [`Flow`]
/// carries the reason. Each connection publishes the receiver's request,
/// which the capture follows; the frame that opens a connection is dropped,
/// since it was captured before the request was known.
///
/// A receiver that pauses the stream (its lighting is off) stops the
/// capture through [`Flow::wanted`]; idle ticks keep reading its control
/// messages, so a resume starts it again. A receiver that pauses and then
/// closes (lighting stopped) leaves the capture stopped until a receiver
/// connects again; one that vanishes without pausing (a crash, the
/// network) leaves it running.
///
/// An encoder failure (a cause of type [`H264Error`]: the requested encoder
/// cannot start or failed) is reported as such, and every later connection
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
                self.flow.lock().waiting =
                    Some(format!("waiting for the receiver: {}", chain(&error)));
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
        let mut report = self.flow.lock();
        *report = Report {
            requested: Some(request),
            encoding: Some(slim_encoding(request.encoding)),
            ..Report::default()
        };
        self.sink = Some(sink);
    }

    /// The connection ended: wait for the receiver, or remember an encoder
    /// failure. A receiver that paused before closing stays paused.
    fn lost(&mut self, error: &Error) {
        let Some(sink) = self.sink.take() else {
            return;
        };
        let mut report = self.flow.lock();
        report.encoding = None;
        report.paused = sink.paused();
        if encoder_failure(error) {
            let reason = chain(error);
            self.failed = Some((sink.request(), reason.clone()));
            report.encoder = Some(reason);
            report.waiting = None;
        } else {
            report.waiting = Some(format!("waiting for the receiver: {}", chain(error)));
        }
    }
}

impl<S: Upstream, C: FnMut() -> Result<S> + Send> FrameSink for Uplink<S, C> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            self.reconnect();
            return Ok(());
        };
        match sink.send(frame) {
            Ok(()) => {
                let paused = sink.paused();
                if !paused {
                    self.flow.sent.fetch_add(1, Ordering::Relaxed);
                }
                let mut report = self.flow.lock();
                report.paused = paused;
                report.waiting = None;
            }
            Err(error) => self.lost(&error),
        }
        Ok(())
    }

    /// Reconnect, or read the receiver's pause and resume while the
    /// capture delivers nothing (it is stopped while paused).
    fn idle(&mut self) -> Result<()> {
        let Some(sink) = &mut self.sink else {
            self.reconnect();
            return Ok(());
        };
        match sink.idle() {
            Ok(()) => self.flow.lock().paused = sink.paused(),
            Err(error) => self.lost(&error),
        }
        Ok(())
    }
}

/// The requested encoder could not start or failed.
fn encoder_failure(error: &Error) -> bool {
    let Error::Adapter {
        source: Some(cause),
        ..
    } = error
    else {
        return false;
    };
    let mut cause: &(dyn std::error::Error + 'static) = &**cause;
    loop {
        if cause.is::<H264Error>() {
            return true;
        }
        match cause.source() {
            Some(next) => cause = next,
            None => return false,
        }
    }
}

/// `Surface<()>`: video commands into the capture, frame flow out as status:
/// `capturing` while frames are delivered, `encoding` while connected,
/// `paused` while the receiver paused the stream, `failure` naming the
/// encoder failure, or else the wait while the receiver is unavailable.
/// `Lighting` is protocol misuse here and stops the worker as failed.
pub struct ForwarderLink<V> {
    link: Link,
    video: V,
    flow: Arc<Flow>,
    sample: Duration,
    heartbeat: Duration,
}

impl<V: VideoFeed> ForwarderLink<V> {
    pub fn new(link: Link, video: V, flow: Arc<Flow>) -> Self {
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
                        ..self.flow.status()
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

impl<V: VideoFeed> Surface<()> for ForwarderLink<V> {
    fn run(self: Box<Self>, (): (), shutdown: Shutdown) -> BoxFuture<'static, Result<()>> {
        Box::pin(self.serve(shutdown))
    }
}
