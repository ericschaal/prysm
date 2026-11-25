//! The worker side of the link: stdio plumbing, why the worker stops, the
//! exit code that follows, and a `main` that wires them together.
//!
//! The worker's first statement is [`take_stdio`]: it keeps a duplicate of
//! the original stdout for [`Event`]s and points fd 1 at stderr, so nothing
//! that prints (argument parsing, logging, vendor libraries) can corrupt the
//! stream; `clippy::print_stdout` is denied in the workspace besides. A
//! reader thread decodes [`Command`]s from stdin; stdin EOF or a bad line
//! stops the worker.
use crate::{Command, Event, Failure, FullStatus, LineReader, LinkError, WorkerStatus, encode};
use prysm_ports::{Error, chain};
use std::future::Future;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{mpsc, watch};

/// Why a worker is stopping. The first reason wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// SIGTERM, SIGINT or stdin EOF: exit 0.
    Requested(&'static str),
    /// Protocol misuse or a lost link: exit 1.
    Failed(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Requested(why) => why,
            Self::Failed(why) => why,
        })
    }
}

/// Records the stop reason and wakes everything waiting for it. Cheap to clone.
#[derive(Debug, Clone)]
pub struct StopHandle(Arc<watch::Sender<Option<Stop>>>);

impl Default for StopHandle {
    fn default() -> Self {
        Self(Arc::new(watch::channel(None).0))
    }
}

impl StopHandle {
    /// Record `stop` unless a reason is already recorded.
    pub fn request(&self, stop: Stop) {
        self.0.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(stop);
            true
        });
    }

    #[must_use]
    pub fn reason(&self) -> Option<Stop> {
        self.0.borrow().clone()
    }

    /// Resolves once any reason is recorded; pass it to `run`.
    pub fn stopped(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut receiver = self.0.subscribe();
        async move {
            let _ = receiver.wait_for(Option::is_some).await;
        }
    }
}

type Sink = Arc<dyn Fn(&[u8]) -> io::Result<()> + Send + Sync>;

/// Writes events to the supervisor. Cheap to clone.
#[derive(Clone)]
pub struct Events {
    sink: Sink,
    stop: StopHandle,
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").finish_non_exhaustive()
    }
}

impl Events {
    /// Send one event. Long texts are shortened until the encoded line fits
    /// (escaping can make a text up to six times longer). A supervisor that
    /// stopped reading means stop.
    pub fn send(&self, event: &Event) {
        let mut limit = MAX_TEXT;
        let line = loop {
            match encode(&fit(event.clone(), limit)) {
                Ok(line) => break line,
                Err(LinkError::Oversized) if limit > 0 => limit /= 2,
                // Unreachable: with no text left every event fits.
                Err(error) => {
                    return eprintln!("cannot send an event: {}", chain(&error));
                }
            }
        };
        if (self.sink)(&line).is_err() {
            self.stop
                .request(Stop::Requested("the supervisor closed the event stream"));
        }
    }
}

const MAX_TEXT: usize = 1024;

/// `text`, cut to `limit` bytes at a character boundary.
fn short(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// `event` with every free text cut to `limit` bytes.
fn fit(event: Event, limit: usize) -> Event {
    let short = |text| short(text, limit);
    let short_failure = |failure: Failure| Failure {
        message: short(failure.message),
        ..failure
    };
    match event {
        Event::Status(WorkerStatus::Full(status)) => {
            Event::Status(WorkerStatus::Full(FullStatus {
                portal: status.portal.map(short),
                failure: status.failure.map(short_failure),
                ..status
            }))
        }
        Event::Status(WorkerStatus::Slim(mut status)) => {
            if let Some(message) = status.link.message_mut() {
                *message = short(std::mem::take(message));
            }
            Event::Status(WorkerStatus::Slim(status))
        }
        Event::Failed(failure) => Event::Failed(short_failure(failure)),
    }
}

/// Commands from the supervisor, in order.
#[derive(Debug)]
pub struct Commands(mpsc::UnboundedReceiver<Command>);

impl Commands {
    /// The next command; `None` once the supervisor closed the link (a stop
    /// reason is recorded by then). Cancel-safe.
    pub async fn next(&mut self) -> Option<Command> {
        self.0.recv().await
    }
}

/// A worker's end of the link.
#[derive(Debug)]
pub struct Link {
    pub commands: Commands,
    pub events: Events,
    pub stop: StopHandle,
}

/// The supervisor's end of an in-memory link, for tests.
#[cfg(any(test, feature = "testkit"))]
#[derive(Debug)]
pub struct Peer {
    commands: Option<mpsc::UnboundedSender<Command>>,
    events: mpsc::UnboundedReceiver<Event>,
    stop: StopHandle,
}

#[cfg(any(test, feature = "testkit"))]
impl Peer {
    pub fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    /// Like closing the worker's stdin.
    pub fn close(&mut self) {
        self.commands = None;
        self.stop.request(Stop::Requested("stdin closed"));
    }

    /// The next event, or `None` once the worker dropped its link.
    pub async fn event(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// The next event matching `want` (`what` describes it), skipping the
    /// others, within five seconds.
    ///
    /// # Panics
    /// None came in time, or the worker dropped its link first.
    pub async fn wait_for(&mut self, what: &str, want: impl Fn(&Event) -> bool) -> Event {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, self.event()).await {
                Ok(Some(event)) if want(&event) => return event,
                Ok(Some(_)) => {}
                Ok(None) => panic!("the link closed before {what}"),
                Err(_) => panic!("no {what} within 5 s"),
            }
        }
    }
}

impl Link {
    /// An in-memory link and its supervisor end, for tests.
    #[cfg(any(test, feature = "testkit"))]
    #[must_use]
    pub fn memory() -> (Self, Peer) {
        let stop = StopHandle::default();
        let (commands, inbox) = mpsc::unbounded_channel();
        let (sent, events) = mpsc::unbounded_channel();
        let sink: Sink = Arc::new(move |line: &[u8]| {
            let event = crate::decode(line).map_err(io::Error::other)?;
            sent.send(event)
                .map_err(|_| io::ErrorKind::BrokenPipe.into())
        });
        let link = Self {
            commands: Commands(inbox),
            events: Events {
                sink,
                stop: stop.clone(),
            },
            stop: stop.clone(),
        };
        let peer = Peer {
            commands: Some(commands),
            events,
            stop,
        };
        (link, peer)
    }

    /// The link over the taken-over stdio: events go to the original stdout,
    /// and a thread reads commands from stdin.
    ///
    /// # Errors
    /// The thread cannot start.
    pub fn stdio(stdio: Stdio) -> io::Result<Self> {
        let out = Mutex::new(stdio.events);
        let sink: Sink = Arc::new(move |line: &[u8]| {
            let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
            out.write_all(line)?;
            out.flush()
        });
        let stop = StopHandle::default();
        let (commands, inbox) = mpsc::unbounded_channel();
        let reader_stop = stop.clone();
        std::thread::Builder::new()
            .name("prysm-link".into())
            .spawn(move || read_commands(io::stdin().lock(), &commands, &reader_stop))?;
        Ok(Self {
            commands: Commands(inbox),
            events: Events {
                sink,
                stop: stop.clone(),
            },
            stop,
        })
    }
}

/// The process's original stdout, kept for events after [`take_stdio`].
#[derive(Debug)]
pub struct Stdio {
    events: std::fs::File,
}

impl Stdio {
    /// Keep a duplicate of the original stdout (closed on exec) and point
    /// fd 1 at stderr.
    ///
    /// # Errors
    /// The descriptors cannot be duplicated.
    fn take() -> io::Result<Self> {
        use nix::fcntl::{FcntlArg, FdFlag, fcntl};
        let original = nix::unistd::dup(io::stdout())?;
        fcntl(&original, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        nix::unistd::dup2_stdout(io::stderr())?;
        Ok(Self {
            events: original.into(),
        })
    }
}

/// Take over stdio for the program `name` (see the module documentation);
/// prints the error and exits 1 when it cannot. The only way
/// to a [`Stdio`]: called once, as the worker's first statement, since a
/// second call would keep stderr as the events' stdout.
#[must_use]
pub fn take_stdio(name: &'static str) -> Stdio {
    Stdio::take().unwrap_or_else(|error| {
        eprintln!("{name}: cannot take over stdio: {error}");
        std::process::exit(1)
    })
}

fn read_commands(
    input: impl io::BufRead,
    commands: &mpsc::UnboundedSender<Command>,
    stop: &StopHandle,
) {
    let mut reader = LineReader::new(input);
    loop {
        match reader.read::<Command>() {
            Ok(Some(command)) => {
                if commands.send(command).is_err() {
                    return;
                }
            }
            Ok(None) => return stop.request(Stop::Requested("stdin closed")),
            Err(error) => {
                return stop.request(Stop::Failed(format!(
                    "bad command from the supervisor: {}",
                    chain(&error)
                )));
            }
        }
    }
}

/// The exit code for how a worker ended: 0 for a stop asked for (or none),
/// 1 for anything else. The supervisor gives it no meaning: a worker says
/// why it fails ([`Event::Failed`]), and an exit without it is a worker
/// lost.
#[must_use]
pub fn exit_code(stop: Option<&Stop>, result: &prysm_ports::Result<()>) -> u8 {
    match (stop, result) {
        (Some(Stop::Failed(_)), _) | (_, Err(_)) => 1,
        (Some(Stop::Requested(_)) | None, Ok(())) => 0,
    }
}

/// Stop with the supervisor: its death sends this process SIGTERM
/// (`PR_SET_PDEATHSIG` follows the spawning thread, which is the
/// supervisor's long-lived loop thread). SIGTERM, not SIGKILL: the worker
/// then runs its normal stop and releases the capture tap; a process killed
/// inside a native capture call can leave the TV's capture locked until a
/// reboot.
///
/// Called before [`main`] installs its signal handlers; a SIGTERM that
/// arrives in between takes the default action and ends the process before
/// `body` has opened anything.
///
/// What bounds a worker that outlives its supervisor (its stop hangs):
/// nothing SIGKILLs it any more. Its own stop deadline (the host's
/// `stop_deadline`) ends the session as `Faulted`, and the full worker's
/// shutdown grace bounds how long it waits for an engine call stuck in an
/// adapter before abandoning it; the runtime is shut down without waiting
/// for such a call, so `main` returns and the process exits. Until it has exited it holds the capture
/// adapter's cross-process lock (and the full worker its portal port), so a
/// new worker cannot open a tap meanwhile: it fails, and its supervisor
/// asks the person. A thread stuck in the kernel keeps the process, and so the lock,
/// until it returns.
///
/// # Errors
/// The signal cannot be set, or the supervisor is already gone.
pub fn guard_parent() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGTERM)
            .map_err(|error| format!("cannot set the parent-death signal: {error}"))?;
        if nix::unistd::getppid() == nix::unistd::Pid::from_raw(1) {
            return Err("the supervisor already exited".into());
        }
    }
    Ok(())
}

/// The worker's arguments, parsed by clap for the program `name`. Prints help
/// (to stderr after [`take_stdio`]) and exits 0 on `--help`; prints the error
/// and exits 1 on arguments the worker does not accept (a packaging bug).
#[must_use]
pub fn args<A: clap::Parser>(name: &'static str, about: &'static str) -> A {
    let mut command = A::command().name(name).bin_name(name).about(about);
    let parsed = command
        .try_get_matches_from_mut(std::env::args_os())
        .and_then(|matches| A::from_arg_matches(&matches))
        .map_err(|error| error.format(&mut command));
    parsed.unwrap_or_else(|error| {
        let _ = error.print();
        std::process::exit(i32::from(error.use_stderr()))
    })
}

/// Link to the supervisor over `stdio` (see [`take_stdio`]), run `body` with
/// the parsed `options` (see [`args`]) on a current-thread runtime until it
/// returns, and exit with the code for how it
/// ended. An error from `body`, or a runtime that cannot start, is sent as a
/// final [`Event::Failed`] first; a stop reason alone (protocol misuse, a lost
/// link) is not, and nothing is sent before the link exists.
pub fn main<A, F, Fut>(name: &'static str, stdio: Stdio, options: A, body: F) -> ExitCode
where
    F: FnOnce(A, Link) -> Fut,
    Fut: Future<Output = prysm_ports::Result<()>>,
{
    if let Err(message) = guard_parent() {
        eprintln!("{name}: {message}");
        return ExitCode::FAILURE;
    }
    let link = match Link::stdio(stdio) {
        Ok(link) => link,
        Err(error) => {
            eprintln!("{name}: cannot set up the supervisor link: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (stop, events) = (link.stop.clone(), link.events.clone());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let error = Error::adapter("cannot start the async runtime", error);
            eprintln!("{name}: {}", chain(&error));
            events.send(&Event::Failed(Failure::of(&error)));
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        watch_signals(&stop)?;
        body(options, link).await
    });
    // Dropping the runtime would wait for an abandoned blocking adapter call.
    runtime.shutdown_background();
    let code = exit_code(stop.reason().as_ref(), &result);
    if let Err(error) = &result {
        eprintln!("{name}: {}", chain(error));
        events.send(&Event::Failed(Failure::of(error)));
    }
    match stop.reason() {
        Some(why) => eprintln!("{name}: {why}; exit {code}"),
        None => eprintln!("{name}: exit {code}"),
    }
    ExitCode::from(code)
}

fn watch_signals(stop: &StopHandle) -> prysm_ports::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let failed = |error| Error::adapter("cannot install signal handlers", error);
    let mut interrupt = signal(SignalKind::interrupt()).map_err(failed)?;
    let mut terminate = signal(SignalKind::terminate()).map_err(failed)?;
    let stop = stop.clone();
    tokio::spawn(async move {
        let why = tokio::select! {
            _ = interrupt.recv() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
        };
        stop.request(Stop::Requested(why));
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SlimLink, SlimStatus};
    use prysm_ports::SessionPhase;

    #[test]
    fn exit_codes_follow_the_stop_reason_and_result() {
        let ok = Ok(());
        let faulted = Err(Error::Faulted(SessionPhase::ClosingSource));
        let failed = Err(Error::other("no receiver"));
        let requested = Stop::Requested("x");
        let misused = Stop::Failed("x".into());
        let rows = [
            (None, &ok, 0),
            (Some(&requested), &ok, 0),
            (Some(&misused), &ok, 1),
            (None, &failed, 1),
            (None, &faulted, 1),
            (Some(&requested), &faulted, 1),
            (Some(&misused), &faulted, 1),
        ];
        for (stop, result, code) in rows {
            assert_eq!(exit_code(stop, result), code, "{stop:?}, {result:?}");
        }
    }

    #[test]
    fn first_stop_reason_wins() {
        let stop = StopHandle::default();
        assert_eq!(stop.reason(), None);
        stop.request(Stop::Failed("tap".into()));
        stop.request(Stop::Requested("SIGTERM"));
        assert_eq!(stop.reason(), Some(Stop::Failed("tap".into())));
    }

    #[test]
    fn reader_forwards_commands_then_stops_on_eof_or_garbage() {
        let (sender, mut inbox) = mpsc::unbounded_channel();
        let stop = StopHandle::default();
        read_commands(&b"{\"type\":\"reset_video\"}\n"[..], &sender, &stop);
        assert_eq!(inbox.try_recv().unwrap(), Command::ResetVideo {});
        assert_eq!(stop.reason(), Some(Stop::Requested("stdin closed")));

        let stop = StopHandle::default();
        read_commands(&b"{\"type\":\"reboot\"}\n"[..], &sender, &stop);
        assert!(matches!(stop.reason(), Some(Stop::Failed(_))));
        let stop = StopHandle::default();
        read_commands(&b"{\"type\":\"reset_video\"}"[..], &sender, &stop);
        assert!(matches!(stop.reason(), Some(Stop::Failed(why)) if why.contains("ended inside")));
    }

    #[tokio::test]
    async fn memory_link_delivers_both_ways_and_close_requests_stop() {
        let (mut link, mut peer) = Link::memory();
        peer.send(Command::ResetVideo {});
        assert_eq!(link.commands.next().await, Some(Command::ResetVideo {}));
        let long = |text: &str| Failure {
            escalation: prysm_ports::Escalation::Restart,
            message: text.repeat(10_000),
        };
        link.events.send(&Event::Failed(long("x")));
        let Some(Event::Failed(failure)) = peer.event().await else {
            panic!("expected a failure");
        };
        assert!(failure.message.len() < 2000, "long messages are shortened");
        assert_eq!(failure.escalation, prysm_ports::Escalation::Restart);
        link.events
            .send(&Event::Status(WorkerStatus::Full(FullStatus {
                capturing: false,
                fps: None,
                target: prysm_ports::LightingTarget::Off,
                lights: None,
                applied: 0,
                portal: Some("z".repeat(10_000)),
                failure: Some(long("y")),
            })));
        let Some(Event::Status(WorkerStatus::Full(full))) = peer.event().await else {
            panic!("expected a full status");
        };
        assert!(full.portal.unwrap().len() < 2000);
        assert!(full.failure.unwrap().message.len() < 2000);
        link.events
            .send(&Event::Status(WorkerStatus::Slim(SlimStatus {
                capturing: false,
                link: SlimLink::Replaced {
                    message: "w".repeat(10_000),
                },
            })));
        let Some(Event::Status(WorkerStatus::Slim(mut slim))) = peer.event().await else {
            panic!("expected a slim status");
        };
        assert!(slim.link.message_mut().unwrap().len() < 2000);
        peer.close();
        assert_eq!(link.commands.next().await, None);
        link.stop.stopped().await;
        assert_eq!(link.stop.reason(), Some(Stop::Requested("stdin closed")));
    }

    /// Escaping can make a text six times longer (`\u0001`); every event
    /// shape still goes out as one whole line within the bound, never a
    /// stand-in event.
    #[test]
    fn every_event_fits_one_line_whatever_its_texts_escape_to() {
        use crate::{MAX_LINE, decode};
        use prysm_ports::StreamEncoding;
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sent = Arc::clone(&lines);
        let events = Events {
            sink: Arc::new(move |line: &[u8]| {
                sent.lock().unwrap().push(line.to_vec());
                Ok(())
            }),
            stop: StopHandle::default(),
        };
        for unit in ["\"", "\\", "\u{1}", "\n", "é", "😀", "x"] {
            for length in [MAX_TEXT - 1, MAX_TEXT, MAX_TEXT + 1, 10_000] {
                let text = unit.repeat(length.div_ceil(unit.len()));
                let failure = || Failure {
                    escalation: prysm_ports::Escalation::Ask(prysm_ports::Fix::ChangeSettings),
                    message: text.clone(),
                };
                for event in [
                    Event::Status(WorkerStatus::Full(FullStatus {
                        capturing: true,
                        fps: Some(u16::MAX),
                        target: prysm_ports::LightingTarget::Effects,
                        lights: Some(prysm_ports::OutputIssue::OtherSender),
                        applied: u64::MAX,
                        portal: Some(text.clone()),
                        failure: Some(failure()),
                    })),
                    Event::Status(WorkerStatus::Slim(SlimStatus {
                        capturing: true,
                        link: SlimLink::EncoderFailed {
                            message: text.clone(),
                        },
                    })),
                    Event::Status(WorkerStatus::Slim(SlimStatus {
                        capturing: true,
                        link: SlimLink::Sending {
                            encoding: StreamEncoding::H264Software,
                            fps: u16::MAX,
                        },
                    })),
                    Event::Failed(failure()),
                ] {
                    events.send(&event);
                    let line = lines.lock().unwrap().pop().expect("a line");
                    assert!(line.len() <= MAX_LINE, "{} bytes", line.len());
                    let back: Event = decode(&line).unwrap();
                    assert_eq!(
                        std::mem::discriminant(&back),
                        std::mem::discriminant(&event)
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn a_closed_event_stream_requests_stop() {
        let (link, peer) = Link::memory();
        drop(peer);
        link.events
            .send(&Event::Status(WorkerStatus::Slim(SlimStatus::default())));
        assert!(matches!(link.stop.reason(), Some(Stop::Requested(_))));
    }
}
