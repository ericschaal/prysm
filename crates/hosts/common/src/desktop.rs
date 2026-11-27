use crate::args::{Options, parse};
use crate::{assets, stops_in_time};
use prysm_capture_file::FileSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_output_wled::Wled;
use prysm_ports::{
    Error, Handoff, LightingTarget, OutputIssue, Result, SessionState, Snapshot, SourceFactory,
    chain,
};
use prysm_runtime::logging::{self, Logs};
use prysm_runtime::{Builder, EngineHandle, NullOutput, Prysm, STOP_DEADLINE, Shutdown, Surface};
use prysm_settings_file::SettingsFile;
use prysm_web_portal::{Access, WebPortal, lan_url};
use std::fmt;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{self, IsTerminal, Write};
use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;

/// The exit code after the host exited at once on a second SIGINT or
/// SIGTERM, without finishing shutdown; the same for either signal.
pub const INTERRUPTED: u8 = 130;

/// What a host executable is.
#[derive(Debug, Clone, Copy)]
pub struct Program {
    /// Prefixes every status and log line.
    pub name: &'static str,
    pub about: &'static str,
}

const _: () = assert!(stops_in_time(prysm_capture_file::STOP, STOP_DEADLINE));
const _: () = assert!(stops_in_time(prysm_capture_remote::STOP, STOP_DEADLINE));

/// Register the desktop capture and output adapters. The portal uses the
/// returned handoff to deliver incoming TV streams to the remote receiver.
pub fn desktop_adapters(prysm: Builder, device: impl SourceFactory) -> (Builder, Handoff) {
    let remote = RemoteReceiver::default();
    let incoming = remote.incoming();
    let prysm = prysm
        .source(device)
        .source(FileSource)
        .source(remote)
        .output(Wled::default())
        .output(NullOutput);
    (prysm, incoming)
}

/// Install the desktop surfaces, then call `host` to run the engine.
/// With `--check-settings`, validate the settings and exit before binding the portal.
/// Exit codes are 0 for success, 1 for failure, 2 for bad arguments and
/// [`INTERRUPTED`] for a second SIGINT or SIGTERM during shutdown.
pub fn main<F, Fut>(program: Program, device: impl SourceFactory, host: F) -> ExitCode
where
    F: FnOnce(Host, Builder) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let Program { name, about } = program;
    let options = parse(name, about, std::env::args_os()).unwrap_or_else(|error| error.exit());
    let terminal = watch::Sender::new(false);
    let shown = Shown(io::stderr().is_terminal().then(|| terminal.subscribe()));
    let logs = match (&options.log_file, &shown.0) {
        (Some(path), _) => match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => Logs::File(file),
            Err(error) => {
                eprintln!("{name}: cannot open log file {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        },
        (None, Some(taken)) => Logs::SharedStderr {
            taken: taken.clone(),
        },
        (None, None) => Logs::Stderr,
    };
    logging::init(name, logs);
    eprintln!("{name}: settings {}", options.settings.display());
    let (prysm, incoming) =
        desktop_adapters(Prysm::builder(SettingsFile::new(&options.settings)), device);
    if options.check_settings {
        return exit_code(name, prysm.check(), "settings OK");
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{name}: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let status = Status {
        name,
        shown,
        out: Out::Stderr,
    };
    let interrupts = match runtime.block_on(async { listen(status.clone()) }) {
        Ok(interrupts) => interrupts,
        Err(error) => {
            eprintln!("{name}: {}", chain(&error));
            return ExitCode::FAILURE;
        }
    };
    let mut again = interrupts.clone();
    let process = Host {
        options,
        status,
        terminal,
        interrupts,
    };
    let ended = runtime.block_on(async {
        tokio::select! {
            biased;
            result = async {
                let prysm = process.surfaces(prysm, incoming)?;
                host(process, prysm).await
            } => Ok(result),
            signal = twice(&mut again) => Err(signal),
        }
    });
    runtime.shutdown_background();
    prysm_capture_file::kill_remaining();
    match ended {
        Ok(result) => exit_code(name, result, "stopped"),
        Err(signal) => {
            eprintln!("{name}: {signal} again, exited without finishing shutdown");
            ExitCode::from(INTERRUPTED)
        }
    }
}

/// Print how `result` ended (`done` when it succeeded) and its exit code.
fn exit_code(name: &str, result: Result<()>, done: &str) -> ExitCode {
    match result {
        Ok(()) => {
            eprintln!("{name}: {done}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{name}: {}", chain(&error));
            ExitCode::FAILURE
        }
    }
}

/// A signal that stops the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Interrupt,
    Terminate,
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        })
    }
}

/// The SIGINT and SIGTERM received so far: the first shuts down, a second
/// exits without waiting for shutdown to finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interrupts {
    Zero,
    Once(Signal),
    Twice(Signal),
}

impl Interrupts {
    #[must_use]
    fn and(self, signal: Signal) -> Self {
        match self {
            Self::Zero => Self::Once(signal),
            Self::Once(_) | Self::Twice(_) => Self::Twice(signal),
        }
    }
}

/// Install the SIGINT and SIGTERM handlers and a task that counts the
/// signals into the returned watch, saying on `status` that shutdown began.
fn listen(status: Status) -> Result<watch::Receiver<Interrupts>> {
    let failed = |error| Error::adapter("cannot install signal handlers", error);
    let mut interrupt = signal(SignalKind::interrupt()).map_err(failed)?;
    let mut terminate = signal(SignalKind::terminate()).map_err(failed)?;
    let (count, interrupts) = watch::channel(Interrupts::Zero);
    tokio::spawn(async move {
        let mut received = Interrupts::Zero;
        while !matches!(received, Interrupts::Twice(_)) {
            let signal = tokio::select! {
                _ = interrupt.recv() => Signal::Interrupt,
                _ = terminate.recv() => Signal::Terminate,
            };
            received = received.and(signal);
            if let Interrupts::Once(signal) = received {
                let status = status.clone();
                tokio::spawn(async move {
                    status
                        .say_when_shown(&format!(
                            "{signal}, shutting down; a second SIGINT or SIGTERM exits at once"
                        ))
                        .await;
                });
            }
            count.send_replace(received);
        }
    });
    Ok(interrupts)
}

/// Resolves on the second signal, with it.
async fn twice(interrupts: &mut watch::Receiver<Interrupts>) -> Signal {
    let second = interrupts
        .wait_for(|received| matches!(received, Interrupts::Twice(_)))
        .await
        .map(|received| *received);
    match second {
        Ok(Interrupts::Twice(signal)) => signal,
        // The listener never ends before the second signal.
        _ => std::future::pending().await,
    }
}

/// The parsed process options of a running host.
#[derive(Debug)]
pub struct Host {
    options: Options,
    status: Status,
    terminal: watch::Sender<bool>,
    interrupts: watch::Receiver<Interrupts>,
}

impl Host {
    /// Resolves on the first SIGINT or SIGTERM (the handlers are installed
    /// before the host starts). A second one exits the process at once with
    /// [`INTERRUPTED`].
    pub fn signals(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut interrupts = self.interrupts.clone();
        async move {
            let _ = interrupts
                .wait_for(|received| *received != Interrupts::Zero)
                .await;
        }
    }

    /// For a surface that draws on the whole terminal: it holds `true` while
    /// it does. Meanwhile, if stderr is a terminal (taken to be the same
    /// one), status, signal and log lines are not written to it; when the
    /// surface lets go, the status lines give the net change since the last
    /// one printed, and the signal line follows.
    #[must_use]
    pub fn terminal(&self) -> watch::Sender<bool> {
        self.terminal.clone()
    }

    fn surfaces(&self, prysm: Builder, incoming: Handoff) -> Result<Builder> {
        let status = self.status.clone();
        let prysm = prysm
            .surface(move |engine, shutdown| status.watch(engine, shutdown))
            .surface(self.start());
        if self.options.no_portal {
            self.status.say("portal disabled");
            return Ok(prysm);
        }
        Ok(prysm.surface(self.open_portal()?.capture_stream(incoming)))
    }

    /// Starts screen sync once if `--start` was given, then ends.
    /// A failure is printed like a status line; the snapshot keeps it too.
    #[must_use]
    fn start(&self) -> impl Surface + use<> {
        let start = self.options.start;
        let status = self.status.clone();
        move |engine: EngineHandle, shutdown: Shutdown| async move {
            if start {
                tokio::select! {
                    () = shutdown.requested() => {}
                    started = engine.set_target(LightingTarget::ScreenSync) => {
                        if let Err(error) = started {
                            status.say_when_shown(&format!("--start failed: {}", chain(&error))).await;
                        }
                    }
                }
            }
            Ok(())
        }
    }

    fn open_portal(&self) -> Result<WebPortal> {
        let options = &self.options;
        let access = options.access();
        let listener = TcpListener::bind(options.listen).map_err(|error| {
            Error::adapter(format!("cannot listen on {}", options.listen), error)
        })?;
        let address = listener
            .local_addr()
            .map_err(|error| Error::adapter("cannot read the portal address", error))?;
        let portal = WebPortal::from_listener(listener, assets::HEADLESS, access.clone())?
            .metrics(options.metrics);
        self.status.say(&format!(
            "{} ({}{})",
            reachable(address),
            describe(&access),
            if options.metrics {
                "; metrics at /metrics"
            } else {
                ""
            }
        ));
        Ok(portal)
    }
}

/// Where the portal can be opened; see [`lan_url`].
fn reachable(address: SocketAddr) -> String {
    let everywhere = address.ip().is_unspecified();
    match lan_url(address) {
        Some(url) if everywhere => format!("portal {url} on all interfaces"),
        Some(url) => format!("portal {url}"),
        None => format!("portal on all interfaces, port {}", address.port()),
    }
}

fn describe(access: &Access) -> String {
    match access {
        Access::SameOrigin => "no password; other sites' pages refused".into(),
        Access::Origins(origins) => format!(
            "no password; other sites' pages refused except {}",
            origins.join(", ")
        ),
        Access::AnyOrigin => "no password; pages from any site allowed".into(),
    }
}

/// Whether a line may be written to stderr now: always, unless stderr is a
/// terminal and a full-screen surface holds the terminal ([`Host::terminal`]).
#[derive(Debug, Clone)]
struct Shown(Option<watch::Receiver<bool>>);

impl Shown {
    /// Runs `write` unless the terminal is taken, holding it meanwhile so a
    /// surface cannot take it in the middle of the line. Whether it ran.
    fn write(&self, write: impl FnOnce()) -> bool {
        match &self.0 {
            Some(taken) => {
                let taken = taken.borrow();
                if !*taken {
                    write();
                }
                !*taken
            }
            None => {
                write();
                true
            }
        }
    }

    /// Resolves once no full-screen surface holds the terminal.
    async fn released(&self) {
        if let Some(taken) = &self.0 {
            let _ = taken.clone().wait_for(|taken| !*taken).await;
        }
    }
}

/// Writes host status lines to stderr while the terminal is available.
#[derive(Debug, Clone)]
struct Status {
    name: &'static str,
    shown: Shown,
    out: Out,
}

/// Where status lines go.
#[derive(Debug, Clone)]
enum Out {
    Stderr,
    /// Each line, for tests.
    #[cfg(test)]
    Lines(tokio::sync::mpsc::UnboundedSender<String>),
}

impl Status {
    /// Before any surface runs, while nothing can hold the terminal.
    fn say(&self, line: &str) {
        self.shown.write(|| self.line(line));
    }

    /// One line, as is; a failed write is ignored (the terminal may be gone).
    fn line(&self, line: &str) {
        match &self.out {
            Out::Stderr => {
                let _ = writeln!(io::stderr(), "{}: {line}", self.name);
            }
            #[cfg(test)]
            Out::Lines(lines) => {
                let _ = lines.send(line.to_owned());
            }
        }
    }

    /// Once no full-screen surface holds the terminal.
    async fn say_when_shown(&self, line: &str) {
        self.shown.released().await;
        self.say(line);
    }

    async fn watch(self, engine: EngineHandle, shutdown: Shutdown) -> Result<()> {
        let mut snapshots = engine.snapshot();
        let mut taken = self.shown.0.clone();
        let mut seen = Seen::default();
        loop {
            // Cloned first: the engine's snapshot is not held while stderr is written.
            let snapshot = snapshots.borrow_and_update().clone();
            self.report(&mut seen, &snapshot);
            let released = async {
                match &mut taken {
                    Some(taken) => taken.changed().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = shutdown.requested() => break,
                changed = snapshots.changed() => if changed.is_err() { break },
                changed = released => if changed.is_err() { taken = None },
            }
        }
        // A full-screen surface lets go of the terminal as it stops.
        self.shown.released().await;
        let snapshot = snapshots.borrow().clone();
        self.report(&mut seen, &snapshot);
        Ok(())
    }

    /// What changed since the last report, unless the terminal is taken:
    /// then nothing is reported, and the next report gives the net change
    /// (intermediate states are not replayed).
    fn report(&self, seen: &mut Seen, snapshot: &Snapshot) {
        self.shown.write(|| {
            for line in seen.update(snapshot) {
                self.line(&line);
            }
        });
    }
}

/// What has already been reported, so only changes are printed.
#[derive(Debug, Default)]
struct Seen {
    /// The `target …, session …` line last printed.
    state: Option<String>,
    failure: Option<String>,
    output_issue: Option<OutputIssue>,
    surface_failures: usize,
}

impl Seen {
    fn update(&mut self, snapshot: &Snapshot) -> Vec<String> {
        let mut lines = Vec::new();
        let mut state = format!(
            "target {}, session {}",
            snapshot.target.as_str(),
            snapshot.session
        );
        if let Some(waiting) = snapshot.control.waiting {
            let prerequisite = match waiting {
                prysm_ports::WaitingFor::Capture => "capture",
                prysm_ports::WaitingFor::Output => "lights",
            };
            state = format!(
                "target {}, waiting for {prerequisite}",
                snapshot.target.as_str()
            );
        } else if let SessionState::Faulted(phase) = snapshot.session {
            state = format!(
                "{state} ({})",
                Error::Faulted(phase).escalation().next_step()
            );
        }
        if self.state.as_ref() != Some(&state) {
            lines.push(state.clone());
            self.state = Some(state);
        }
        if snapshot.device.issue != self.output_issue {
            match snapshot.device.issue {
                Some(issue) => lines.push(format!(
                    "lights not showing Prysm, lighting continues: {issue}"
                )),
                None if matches!(snapshot.session, SessionState::ScreenSync) => {
                    lines.push("lights showing Prysm again".to_owned());
                }
                None => {}
            }
            self.output_issue = snapshot.device.issue;
        }
        let failure = snapshot.session.failure().map(|failure| {
            if snapshot.control.waiting.is_some() {
                format!("lighting waiting: {}", chain(failure))
            } else {
                format!(
                    "lighting failed: {} ({})",
                    chain(failure),
                    failure.escalation().next_step()
                )
            }
        });
        if failure != self.failure {
            if let Some(failure) = &failure {
                lines.push(failure.clone());
            }
            self.failure = failure;
        }
        for failure in snapshot.surface_failures.iter().skip(self.surface_failures) {
            lines.push(format!(
                "surface failed, lighting continues: {} ({})",
                chain(failure),
                failure.escalation().next_step()
            ));
        }
        self.surface_failures = snapshot.surface_failures.len();
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reports_only_changes() {
        let mut seen = Seen::default();
        let mut snapshot = Snapshot::default();
        assert_eq!(seen.update(&snapshot), ["target off, session stopped"]);
        assert!(seen.update(&snapshot).is_empty());

        snapshot.target = LightingTarget::ScreenSync;
        snapshot.session = SessionState::Stopped {
            failure: Some(Error::other("no frames")),
        };
        snapshot.surface_failures.push(Error::other("no terminal"));
        assert_eq!(
            seen.update(&snapshot),
            [
                "target screen_sync, session stopped",
                "lighting failed: no frames (try again)",
                "surface failed, lighting continues: no terminal (try again)",
            ]
        );
        assert!(seen.update(&snapshot).is_empty());

        snapshot.session = SessionState::ScreenSync;
        let _ = seen.update(&snapshot);
        snapshot.device.issue = Some(OutputIssue::Off);
        assert_eq!(
            seen.update(&snapshot),
            ["lights not showing Prysm, lighting continues: switched off on the device"]
        );
        snapshot.device.issue = None;
        assert_eq!(seen.update(&snapshot), ["lights showing Prysm again"]);

        snapshot.session = SessionState::Faulted(prysm_ports::SessionPhase::IdlingOutput);
        assert_eq!(
            seen.update(&snapshot),
            ["target screen_sync, session faulted while idling the output (restart Prysm)"]
        );
    }

    #[test]
    fn nothing_is_written_while_a_full_screen_surface_holds_the_terminal() {
        let terminal = watch::Sender::new(false);
        let shown = Shown(Some(terminal.subscribe()));
        assert!(shown.write(|| {}));
        terminal.send_replace(true);
        let mut wrote = false;
        assert!(!shown.write(|| wrote = true));
        assert!(!wrote);
        terminal.send_replace(false);
        assert!(shown.write(|| wrote = true));
        assert!(wrote);
        terminal.send_replace(true);
        assert!(Shown(None).write(|| {}));
    }

    /// While the terminal is taken nothing is printed; on release, and once
    /// more after shutdown, the net change since the last line is.
    #[tokio::test]
    async fn status_missed_while_the_terminal_is_taken_is_printed_on_release() {
        use prysm_runtime::fakes::{self, CallLog, FakeOutputs, FakeSources, MemoryStore};
        use std::time::Duration;
        let terminal = watch::Sender::new(true);
        let (sent, mut lines) = tokio::sync::mpsc::unbounded_channel();
        let status = Status {
            name: "prysm-test",
            shown: Shown(Some(terminal.subscribe())),
            out: Out::Lines(sent),
        };
        let log = CallLog::new();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let driver = move |engine: EngineHandle, shutdown: Shutdown| async move {
            let mut snapshot = engine.snapshot();
            engine.set_target(LightingTarget::ScreenSync).await?;
            let running = |s: &Snapshot| matches!(s.session, SessionState::ScreenSync);
            let _ = snapshot.wait_for(running).await;
            assert!(lines.try_recv().is_err());
            terminal.send_replace(false);
            assert_eq!(
                lines.recv().await.unwrap(),
                "target screen_sync, session screen_sync"
            );
            terminal.send_replace(true);
            engine.set_target(LightingTarget::Off).await?;
            let off = |s: &Snapshot| matches!(s.session, SessionState::Stopped { .. });
            let _ = snapshot.wait_for(off).await;
            let _ = stop.send(());
            shutdown.requested().await;
            assert!(lines.try_recv().is_err());
            terminal.send_replace(false);
            assert_eq!(lines.recv().await.unwrap(), "target off, session stopped");
            Ok(())
        };
        let run = Prysm::builder(MemoryStore::with(fakes::settings()))
            .source(FakeSources(log.clone()))
            .output(FakeOutputs(log))
            .surface(move |engine, shutdown| status.watch(engine, shutdown))
            .surface(driver)
            .run(async {
                let _ = stopped.await;
            });
        tokio::time::timeout(Duration::from_secs(10), run)
            .await
            .expect("host did not stop")
            .unwrap();
    }

    #[test]
    fn a_second_signal_exits_whatever_came_first() {
        use Interrupts::{Once, Twice, Zero};
        use Signal::{Interrupt, Terminate};
        for (received, signal, then) in [
            (Zero, Interrupt, Once(Interrupt)),
            (Zero, Terminate, Once(Terminate)),
            (Once(Interrupt), Interrupt, Twice(Interrupt)),
            (Once(Interrupt), Terminate, Twice(Terminate)),
            (Once(Terminate), Interrupt, Twice(Interrupt)),
            (Once(Terminate), Terminate, Twice(Terminate)),
            (Twice(Interrupt), Terminate, Twice(Terminate)),
            (Twice(Terminate), Interrupt, Twice(Interrupt)),
        ] {
            assert_eq!(received.and(signal), then, "{received:?} + {signal:?}");
        }
    }
}
