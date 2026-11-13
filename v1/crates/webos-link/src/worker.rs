//! The worker side of the link: stdio plumbing, why the worker stops, the
//! exit code that follows, and a `main` that wires them together.
//!
//! At startup the worker keeps a duplicate of its original stdout for
//! [`Event`]s and points fd 1 at stderr, so vendor libraries that print
//! cannot corrupt the stream. A reader thread decodes [`Command`]s from stdin;
//! stdin EOF or a bad line stops the worker.
use crate::{Command, Event, LineReader, WorkerFailure, WorkerStatus, encode, exit};
use prysm_ports::{Error, chain};
use std::future::Future;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{mpsc, watch};

/// Why a worker is stopping. The first reason wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// SIGTERM, SIGINT or stdin EOF: exit [`exit::STOPPED`].
    Requested(&'static str),
    /// A native resource is unconfirmed: exit [`exit::FAULTED`].
    Faulted(String),
    /// Protocol misuse or a lost link: exit [`exit::FAILED`].
    Failed(String),
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
    /// Send one event. Long texts are shortened to fit a line. A supervisor
    /// that stopped reading means stop.
    pub fn send(&self, event: &Event) {
        let line = encode(&fit(event.clone())).unwrap_or_else(|_| {
            // Unreachable after `fit`; never send a partial line.
            b"{\"type\":\"faulted\",\"cause\":\"event too large\"}\n".to_vec()
        });
        if (self.sink)(&line).is_err() {
            self.stop
                .request(Stop::Requested("the supervisor closed the event stream"));
        }
    }
}

const MAX_TEXT: usize = 1024;

fn fit(event: Event) -> Event {
    let short = |text: String| {
        if text.len() <= MAX_TEXT {
            return text;
        }
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &text[..end])
    };
    match event {
        Event::Status(status) => Event::Status(WorkerStatus {
            portal: status.portal.map(short),
            failure: status.failure.map(|failure| WorkerFailure {
                message: short(failure.message),
                ..failure
            }),
            ..status
        }),
        Event::Faulted { cause } => Event::Faulted {
            cause: short(cause),
        },
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
#[derive(Debug)]
pub struct Peer {
    commands: Option<mpsc::UnboundedSender<Command>>,
    events: mpsc::UnboundedReceiver<Event>,
    stop: StopHandle,
}

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

    /// Wait up to five seconds for an event matching `want`.
    pub async fn wait_for(&mut self, want: impl Fn(&Event) -> bool) -> Option<Event> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, self.event()).await {
                Ok(Some(event)) if want(&event) => return Some(event),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return None,
            }
        }
    }
}

impl Link {
    /// An in-memory link and its supervisor end, for tests.
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

    /// Take over the process's stdio: events go to the original stdout, fd 1
    /// becomes stderr, and a thread reads commands from stdin.
    ///
    /// # Errors
    /// The descriptors cannot be duplicated or the thread cannot start.
    pub fn stdio() -> io::Result<Self> {
        use nix::fcntl::{FcntlArg, FdFlag, fcntl};
        let original = nix::unistd::dup(io::stdout())?;
        fcntl(&original, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        nix::unistd::dup2_stdout(io::stderr())?;
        let out = Mutex::new(std::fs::File::from(original));
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

/// The exit code for how a worker ended.
#[must_use]
pub fn exit_code(stop: Option<&Stop>, result: &prysm_ports::Result<()>) -> u8 {
    match (stop, result) {
        (Some(Stop::Faulted(_)), _) | (_, Err(Error::Faulted(_))) => exit::FAULTED,
        (Some(Stop::Failed(_)), _) | (_, Err(_)) => exit::FAILED,
        _ => exit::STOPPED,
    }
}

/// Die with the supervisor. `PR_SET_PDEATHSIG` follows the spawning thread,
/// which is the supervisor's long-lived loop thread.
///
/// # Errors
/// The signal cannot be set, or the supervisor is already gone.
pub fn guard_parent() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)
            .map_err(|error| format!("cannot set the parent-death signal: {error}"))?;
        if nix::unistd::getppid() == nix::unistd::Pid::from_raw(1) {
            return Err("the supervisor already exited".into());
        }
    }
    Ok(())
}

/// Parse argv, take over stdio, run `body` on a current-thread runtime until
/// it returns, and exit with the code for how it ended. On failure a final
/// status carries the cause.
pub fn main<A, F, Fut>(
    name: &'static str,
    usage: &str,
    parse: impl FnOnce(&[String]) -> Result<A, String>,
    body: F,
) -> ExitCode
where
    F: FnOnce(A, Link) -> Fut,
    Fut: Future<Output = prysm_ports::Result<()>>,
{
    let args: Option<Vec<String>> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().ok())
        .collect();
    let parsed = args
        .ok_or_else(|| "arguments must be UTF-8".to_owned())
        .and_then(|args| parse(&args));
    let options = match parsed {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{name}: {message}\n\nusage: {usage}");
            return ExitCode::from(exit::USAGE);
        }
    };
    if let Err(message) = guard_parent() {
        eprintln!("{name}: {message}");
        return ExitCode::from(exit::FAILED);
    }
    let link = match Link::stdio() {
        Ok(link) => link,
        Err(error) => {
            eprintln!("{name}: cannot set up the supervisor link: {error}");
            return ExitCode::from(exit::FAILED);
        }
    };
    let (stop, events) = (link.stop.clone(), link.events.clone());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{name}: cannot start the async runtime: {error}");
            return ExitCode::from(exit::FAILED);
        }
    };
    let result = runtime.block_on(async {
        watch_signals(&stop)?;
        body(options, link).await
    });
    let code = exit_code(stop.reason().as_ref(), &result);
    if let Err(error) = &result {
        let failure = chain(error);
        eprintln!("{name}: {failure}");
        events.send(&Event::Status(WorkerStatus {
            failure: Some(WorkerFailure::other(failure)),
            ..WorkerStatus::default()
        }));
    }
    match stop.reason() {
        Some(Stop::Requested(why)) => eprintln!("{name}: {why}; exit {code}"),
        Some(Stop::Faulted(why) | Stop::Failed(why)) => eprintln!("{name}: {why}; exit {code}"),
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
    use prysm_ports::SessionPhase;

    #[test]
    fn exit_codes_follow_the_stop_reason_and_result() {
        let ok = Ok(());
        let faulted = Err(Error::Faulted(SessionPhase::ClosingSource));
        let failed = Err(Error::other("no receiver"));
        assert_eq!(exit_code(None, &ok), exit::STOPPED);
        assert_eq!(exit_code(Some(&Stop::Requested("x")), &ok), exit::STOPPED);
        assert_eq!(exit_code(None, &failed), exit::FAILED);
        assert_eq!(
            exit_code(Some(&Stop::Failed("x".into())), &ok),
            exit::FAILED
        );
        assert_eq!(exit_code(None, &faulted), exit::FAULTED);
        assert_eq!(
            exit_code(Some(&Stop::Faulted("x".into())), &failed),
            exit::FAULTED
        );
        assert_eq!(
            exit_code(Some(&Stop::Requested("x")), &faulted),
            exit::FAULTED
        );
    }

    #[test]
    fn first_stop_reason_wins() {
        let stop = StopHandle::default();
        assert_eq!(stop.reason(), None);
        stop.request(Stop::Faulted("tap".into()));
        stop.request(Stop::Requested("SIGTERM"));
        assert_eq!(stop.reason(), Some(Stop::Faulted("tap".into())));
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
        link.events.send(&Event::Faulted {
            cause: "x".repeat(10_000),
        });
        let Some(Event::Faulted { cause }) = peer.event().await else {
            panic!("expected a fault");
        };
        assert!(cause.len() < 2000, "long causes are shortened");
        peer.close();
        assert_eq!(link.commands.next().await, None);
        link.stop.stopped().await;
        assert_eq!(link.stop.reason(), Some(Stop::Requested("stdin closed")));
    }

    #[tokio::test]
    async fn a_closed_event_stream_requests_stop() {
        let (link, peer) = Link::memory();
        drop(peer);
        link.events.send(&Event::Status(WorkerStatus::default()));
        assert!(matches!(link.stop.reason(), Some(Stop::Requested(_))));
    }
}
