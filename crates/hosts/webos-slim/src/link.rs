//! The forwarder's side of the supervisor link, and the uplink sink that
//! keeps reconnecting to the receiver, follows its request and tells the
//! link whether frames flow.
use classify::{Cause, Heard};
use futures::future::BoxFuture;
use prysm_capture_remote::{RemoteSender, RemoteSettings, SendError};
use prysm_capture_webos::VideoSignal;
use prysm_core::Frame;
use prysm_ports::{Error, Escalation, FrameSink, Result, StreamEncoding};
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

/// How long the uplink waits, after a connection attempt returned, before
/// the next one, without end, in every state without a connection: the
/// receiver is unavailable (its host is not lighting, is unreachable or its
/// name does not resolve yet), stopped the stream, failed with it, or speaks
/// another capture stream version; or this end refused its request. Only
/// what [`classify::heard`] answers with `Err` fails the worker. Each state
/// ends by a step on the computer at a time nothing announces (lighting
/// started again, another encoding chosen, Prysm started or updated, the
/// network back), or by an update of this app, which replaces the worker.
/// So one try every second reaches the receiver within a second of its
/// return, plus the try in flight then. A try costs one TCP connect and one
/// HTTP answer, bounded by [`crate::resolve::RESOLVE_WAIT`] and
/// [`prysm_capture_remote::CONNECT_DEADLINE`] together. The next try is due
/// 1 s after the last one returned, so a host that drops packets is not
/// tried back to back.
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

/// The uplink's state machine: every rule is in [`Report::next`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Report {
    /// The receiver's latest request, kept while it is away: the capture
    /// follows it.
    requested: Option<RemoteSettings>,
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
    Sending(StreamEncoding),
    /// The receiver stopped the stream (its lighting is off, or its capture
    /// settings changed): the capture is released until a receiver takes a
    /// connection again.
    Stopped,
    /// The receiver is unavailable, and why.
    Waiting(String),
    /// Prysm speaks another capture stream version, and which.
    VersionDiffers(String),
    /// The requested encoder failed, and why: that request is refused until
    /// it changes, and the capture is released meanwhile.
    EncoderFailed(String),
    /// The receiver's source failed with this stream, and how it said so:
    /// the capture is released until a receiver takes a connection again.
    ReceiverFailed(String),
}

/// What happened to the uplink.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UplinkEvent {
    /// A connection opened with the receiver's request.
    Connected(RemoteSettings),
    /// A connection attempt failed, or the open connection ended.
    Heard(Heard),
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
    /// The state after `event`, and what to do with the connection. A
    /// connection with the refused request is refused again; any other is
    /// sent on. Another version replaces whatever was shown (it came from
    /// the Prysm before its update); an unavailable receiver does not
    /// replace a stopped stream, an encoder failure, a failed receiver or
    /// another version, which are still the news.
    fn next(&self, event: UplinkEvent) -> (Self, Action) {
        use UplinkState::{
            EncoderFailed, ReceiverFailed, Sending, Stopped, VersionDiffers, Waiting,
        };
        let shown = |uplink| Self {
            uplink,
            ..self.clone()
        };
        match (&self.uplink, event) {
            (
                Stopped | EncoderFailed(_) | ReceiverFailed(_) | VersionDiffers(_),
                UplinkEvent::Heard(Heard::Absent(_)),
            ) => (self.clone(), Action::Drop),
            (_, UplinkEvent::Connected(request)) => match &self.refused {
                Some((refused, reason)) if *refused == request => (
                    shown(EncoderFailed(reason.clone())),
                    Action::Refuse(reason.clone()),
                ),
                _ => {
                    let next = Self {
                        requested: Some(request),
                        uplink: Sending(request.encoding),
                        refused: None,
                    };
                    (next, Action::Keep)
                }
            },
            (_, UplinkEvent::Heard(Heard::OtherVersion(why))) => {
                (shown(VersionDiffers(why)), Action::Drop)
            }
            (_, UplinkEvent::Heard(Heard::Absent(why))) => (shown(Waiting(why)), Action::Drop),
            (_, UplinkEvent::Heard(Heard::Refused { request, reason })) => {
                let next = Self {
                    uplink: EncoderFailed(reason.clone()),
                    refused: Some((request, reason)),
                    ..self.clone()
                };
                (next, Action::Drop)
            }
            (_, UplinkEvent::Heard(Heard::Stopped)) => (shown(Stopped), Action::Drop),
            (_, UplinkEvent::Heard(Heard::Failed(why))) => {
                (shown(ReceiverFailed(why)), Action::Drop)
            }
        }
    }

    /// See [`Flow::wanted`].
    fn wanted(&self) -> Option<RemoteSettings> {
        match self.uplink {
            UplinkState::Stopped
            | UplinkState::EncoderFailed(_)
            | UplinkState::ReceiverFailed(_)
            | UplinkState::VersionDiffers(_) => None,
            UplinkState::Starting | UplinkState::Sending(_) | UplinkState::Waiting(_) => {
                self.requested
            }
        }
    }

    /// The link as the status reports it, `fps` frames sent per second
    /// while sending, refused while sending and the capture says the TV
    /// refuses its video. A stopped stream is paused, not a failure.
    fn link(&self, fps: u16, refused: bool) -> SlimLink {
        match &self.uplink {
            UplinkState::Starting => SlimLink::Starting {},
            UplinkState::Sending(_) if refused => SlimLink::Refused {},
            UplinkState::Sending(encoding) => SlimLink::Sending {
                encoding: *encoding,
                fps,
            },
            UplinkState::Stopped => SlimLink::Paused {},
            UplinkState::Waiting(why) => SlimLink::Waiting {
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

    fn link(&self, fps: u16) -> SlimLink {
        self.lock().link(fps, self.refused.load(Ordering::Relaxed))
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
/// reconnecting; it fails only where waiting cannot help
/// ([`classify::heard`]). The receiver only listens while its host is
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
/// over) fails the uplink, and so the worker: the TV asks the person to try
/// again. The replaced connection is always the live one: the uplink drops
/// a failed connection, closing its socket, before it connects again, so a
/// TV that reconnects after a network drop never hears its own old
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
        let (event, sink) = match connected {
            Ok(sink) => (UplinkEvent::Connected(sink.request()), Some(sink)),
            Err(error) => match classify::heard(Cause::Attempt(&error)) {
                Ok(heard) => (UplinkEvent::Heard(heard), None),
                Err(escalation) => return Err(ended(error, escalation)),
            },
        };
        match (self.flow.apply(event), sink) {
            (Action::Keep, sink) => self.sink = sink,
            (Action::Refuse(reason), Some(mut sink)) => sink.refuse_encoding(&reason),
            (Action::Refuse(_) | Action::Drop, _) => {}
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
                Ok(heard) => self.flow.apply(UplinkEvent::Heard(heard)),
                Err(escalation) => return Err(ended(error.into(), escalation)),
            };
        }
        self.check();
        Ok(())
    }

    /// A connection is open exactly while sending.
    fn check(&self) {
        debug_assert_eq!(
            self.sink.is_some(),
            matches!(self.flow.lock().uplink, UplinkState::Sending(_))
        );
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
        EncoderFailed, ReceiverFailed, Sending, Starting, Stopped, VersionDiffers, Waiting,
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
    /// same from any state; an absent receiver is heard alike after an
    /// attempt and after a lost connection.
    #[test]
    fn every_uplink_state_takes_every_event() {
        let connected = |request| UplinkEvent::Connected(request);
        let heard = UplinkEvent::Heard;
        let absent = || heard(Heard::Absent(text("w")));
        let version = || heard(Heard::OtherVersion(text("v")));
        let refused = || {
            heard(Heard::Refused {
                request: OLD,
                reason: text("e"),
            })
        };
        let stopped = || heard(Heard::Stopped);
        let failed = || heard(Heard::Failed(text("f")));
        let software = Sending(StreamEncoding::H264Software);
        let hardware = Sending(StreamEncoding::H264Hardware);
        let raw = Sending(StreamEncoding::Raw);
        let (keep, drop, refuse) = (Action::Keep, Action::Drop, Action::Refuse(text("busy")));
        #[rustfmt::skip]
        let table = [
            (Starting, connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (Starting, connected(FAILED), hardware, keep.clone(), Some(FAILED)),
            (Starting, absent(), Waiting(text("w")), drop.clone(), None),
            (Starting, version(), VersionDiffers(text("v")), drop.clone(), None),
            (Starting, refused(), EncoderFailed(text("e")), drop.clone(), None),
            (Starting, stopped(), Stopped, drop.clone(), None),
            (Starting, failed(), ReceiverFailed(text("f")), drop.clone(), None),

            (raw.clone(), connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (raw.clone(), connected(FAILED), EncoderFailed(text("busy")), refuse.clone(), None),
            (raw.clone(), absent(), Waiting(text("w")), drop.clone(), Some(OLD)),
            (raw.clone(), version(), VersionDiffers(text("v")), drop.clone(), None),
            (raw.clone(), refused(), EncoderFailed(text("e")), drop.clone(), None),
            (raw.clone(), stopped(), Stopped, drop.clone(), None),
            (raw.clone(), failed(), ReceiverFailed(text("f")), drop.clone(), None),

            (Stopped, connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (Stopped, connected(FAILED), EncoderFailed(text("busy")), refuse.clone(), None),
            (Stopped, absent(), Stopped, drop.clone(), None),
            (Stopped, version(), VersionDiffers(text("v")), drop.clone(), None),
            (Stopped, refused(), EncoderFailed(text("e")), drop.clone(), None),
            (Stopped, stopped(), Stopped, drop.clone(), None),
            (Stopped, failed(), ReceiverFailed(text("f")), drop.clone(), None),

            (Waiting(text("w0")), connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (Waiting(text("w0")), connected(FAILED), EncoderFailed(text("busy")), refuse.clone(), None),
            (Waiting(text("w0")), absent(), Waiting(text("w")), drop.clone(), Some(OLD)),
            (Waiting(text("w0")), version(), VersionDiffers(text("v")), drop.clone(), None),
            (Waiting(text("w0")), refused(), EncoderFailed(text("e")), drop.clone(), None),
            (Waiting(text("w0")), stopped(), Stopped, drop.clone(), None),
            (Waiting(text("w0")), failed(), ReceiverFailed(text("f")), drop.clone(), None),

            (VersionDiffers(text("v0")), connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (VersionDiffers(text("v0")), connected(FAILED), EncoderFailed(text("busy")), refuse.clone(), None),
            (VersionDiffers(text("v0")), absent(), VersionDiffers(text("v0")), drop.clone(), None),
            (VersionDiffers(text("v0")), version(), VersionDiffers(text("v")), drop.clone(), None),
            (VersionDiffers(text("v0")), refused(), EncoderFailed(text("e")), drop.clone(), None),
            (VersionDiffers(text("v0")), stopped(), Stopped, drop.clone(), None),
            (VersionDiffers(text("v0")), failed(), ReceiverFailed(text("f")), drop.clone(), None),

            (EncoderFailed(text("e0")), connected(NEW), software.clone(), keep.clone(), Some(NEW)),
            (EncoderFailed(text("e0")), connected(FAILED), EncoderFailed(text("busy")), refuse.clone(), None),
            (EncoderFailed(text("e0")), absent(), EncoderFailed(text("e0")), drop.clone(), None),
            (EncoderFailed(text("e0")), version(), VersionDiffers(text("v")), drop.clone(), None),
            (EncoderFailed(text("e0")), refused(), EncoderFailed(text("e")), drop.clone(), None),
            (EncoderFailed(text("e0")), stopped(), Stopped, drop.clone(), None),
            (EncoderFailed(text("e0")), failed(), ReceiverFailed(text("f")), drop.clone(), None),

            (ReceiverFailed(text("f0")), connected(NEW), software, keep, Some(NEW)),
            (ReceiverFailed(text("f0")), connected(FAILED), EncoderFailed(text("busy")), refuse, None),
            (ReceiverFailed(text("f0")), absent(), ReceiverFailed(text("f0")), drop.clone(), None),
            (ReceiverFailed(text("f0")), version(), VersionDiffers(text("v")), drop.clone(), None),
            (ReceiverFailed(text("f0")), refused(), EncoderFailed(text("e")), drop.clone(), None),
            (ReceiverFailed(text("f0")), stopped(), Stopped, drop.clone(), None),
            (ReceiverFailed(text("f0")), failed(), ReceiverFailed(text("f")), drop, None),
        ];
        assert_eq!(table.len(), 7 * 7);
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
            let refused = match (&event, &done) {
                (UplinkEvent::Heard(Heard::Refused { request, reason }), _) => {
                    Some((*request, reason.clone()))
                }
                (_, Action::Keep) => None,
                _ => before.refused.clone(),
            };
            assert_eq!(next.refused, refused, "{shown}");
        }
    }

    /// What the status says in each state.
    #[test]
    fn each_state_is_reported_as_its_link() {
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
            (Waiting(text("w")), SlimLink::Waiting { message: text("w") }),
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
            // Refused only while sending.
            let refused = match uplink {
                Sending(_) => SlimLink::Refused {},
                _ => link,
            };
            assert_eq!(from(uplink).link(7, true), refused);
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
    /// last returned, however long it took.
    #[test]
    fn attempts_count_from_the_last_return() {
        let slow = Duration::from_millis(20);
        let (mut uplink, _flow) = Uplink::new(|| -> Result<Never> {
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
}
