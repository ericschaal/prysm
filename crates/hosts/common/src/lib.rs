//! What the hosts share. For every host that serves the portal: its
//! embedded bundles ([`assets`]). For the desktop hosts also the process
//! concerns, so each host `main` is only its builder list: arguments,
//! logging, the portal listener, signals, a status surface that reports to
//! stderr and the `--start` request.
//!
//! ```ignore
//! const PROGRAM: Program = Program { name: "prysm-headless", about: ABOUT };
//!
//! fn main() -> ExitCode {
//!     let (adapters, tv_stream) = prysm_host::desktop_adapters(DeviceSource);
//!     prysm_host::main(PROGRAM, adapters, |host, prysm| async move {
//!         prysm
//!             .surface(host.status())
//!             .surface(host.start())
//!             .surface(host.portal(assets::HEADLESS)?.capture_stream(tv_stream))
//!             .run(host.signals())
//!             .await
//!     })
//! }
//! ```
//!
//! `--check-settings` is handled here: the settings file is validated against
//! the host's adapters and the run closure is never called. For every host
//! with a WLED output, [`stops_in_time`]. Feature `desktop` adds
//! [`desktop_adapters`] and the desktop hosts' `STOP_DEADLINE`. Unix only,
//! as the hosts are.
mod args;
pub mod assets;

use args::{Options, parse};
#[cfg(feature = "desktop")]
pub use desktop::desktop_adapters;
/// The desktop hosts' stop deadline: the runtime's own.
#[cfg(feature = "desktop")]
pub use prysm_runtime::STOP_DEADLINE;

use prysm_output_wled::{IDLE_LIMIT, STOP_WAIT};
use prysm_ports::SourceStop;
use prysm_runtime::TICK;
use std::time::Duration;

/// Whether every stop of a session with `source` and a WLED output ends
/// within `deadline`, whatever it meets. An open or an idle of the output in
/// flight ends at once when the stop comes, so a stop waits for the pull in
/// flight (its timeout, [`TICK`], then the rest of a frame or of an end, as
/// `source` says), the source's close and the stop's idle
/// ([`IDLE_LIMIT`]); or, after the session ended on its own and while it
/// idles `Off`, two idles ([`STOP_WAIT`]). Every host with a WLED output
/// asserts it for each source it registers.
#[must_use]
pub const fn stops_in_time(source: SourceStop, deadline: Duration) -> bool {
    let pull = if source.frame.as_millis() > source.end.as_millis() {
        source.frame
    } else {
        source.end
    };
    let closing = TICK
        .saturating_add(pull)
        .saturating_add(source.close)
        .saturating_add(IDLE_LIMIT);
    closing.as_millis() < deadline.as_millis() && STOP_WAIT.as_millis() < deadline.as_millis()
}

#[cfg(feature = "desktop")]
mod desktop {
    use super::stops_in_time;
    use prysm_capture_file::FileSource;
    use prysm_capture_remote::RemoteReceiver;
    use prysm_output_wled::Wled;
    use prysm_ports::{Handoff, SourceFactory};
    use prysm_runtime::{Builder, NullOutput, STOP_DEADLINE};

    const _: () = assert!(stops_in_time(prysm_capture_file::STOP, STOP_DEADLINE));
    const _: () = assert!(stops_in_time(prysm_capture_remote::STOP, STOP_DEADLINE));

    /// What both desktop hosts register: sources `device` (the factory
    /// given, `DeviceSource` in the hosts), `file` and `remote`, outputs
    /// `wled` and `null`. Also the hand-off through which the portal gives a
    /// capture-only TV's stream to the `remote` source
    /// ([`Portal::capture_stream`](crate::Portal::capture_stream)).
    pub fn desktop_adapters(
        device: impl SourceFactory,
    ) -> (impl FnOnce(Builder) -> Builder, Handoff) {
        let remote = RemoteReceiver::default();
        let tv_stream = remote.incoming();
        let adapters = move |prysm: Builder| {
            prysm
                .source(device)
                .source(FileSource)
                .source(remote)
                .output(Wled::default())
                .output(NullOutput)
        };
        (adapters, tv_stream)
    }
}

use futures::future::BoxFuture;
use prysm_ports::{
    Error, Handoff, LightingTarget, OutputIssue, Result, SessionState, Snapshot, chain,
};
use prysm_runtime::logging::{self, Logs};
use prysm_runtime::{Builder, EngineHandle, Prysm, Shutdown, Surface};
use prysm_settings_file::SettingsFile;
use prysm_web_portal::{Access, FrontendAsset, WebPortal, lan_url};
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

/// Parse arguments, install logging, register `adapters` on a builder over
/// the settings file, run `host` with it on a current-thread runtime and map
/// its result to an exit code: 0 after a clean stop or `--help`/`--version`,
/// 1 if it failed, 2 for bad arguments, [`INTERRUPTED`] when a second SIGINT
/// or SIGTERM ended shutdown early. Under `--check-settings` only validates
/// the settings ([`Builder::check`]): 0 if valid, else 1; `host` is not
/// called.
pub fn main<A, F, Fut>(program: Program, adapters: A, host: F) -> ExitCode
where
    A: FnOnce(Builder) -> Builder,
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
    let prysm = adapters(Prysm::builder(SettingsFile::new(&options.settings)));
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
    // Before the engine starts, so a signal that arrives meanwhile is not lost.
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
            result = host(process, prysm) => Ok(result),
            signal = twice(&mut again) => Err(signal),
        }
    });
    // Drops the surfaces' tasks, so a full-screen surface restores the
    // terminal. Dropping the runtime would wait for an abandoned blocking
    // adapter call.
    runtime.shutdown_background();
    // A session abandoned at its stop deadline, or cut short above, still
    // holds its source.
    #[cfg(feature = "desktop")]
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

    /// Reports state changes and surface failures to stderr; see
    /// [`Host::terminal`].
    #[must_use]
    pub fn status(&self) -> Status {
        self.status.clone()
    }

    /// Starts screen sync once if `--start` was given, then ends.
    /// A failure is printed like a status line; the snapshot keeps it too.
    #[must_use]
    pub fn start(&self) -> impl Surface + use<> {
        let start = self.options.start;
        let status = self.status();
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

    /// The web portal on its bound listener, or a surface that does nothing
    /// under `--no-portal`. Binds now, so the actual address is printed.
    ///
    /// # Errors
    /// Bind failure, or an origin `WebPortal` rejects.
    pub fn portal(&self, assets: &'static [FrontendAsset]) -> Result<Portal> {
        if self.options.no_portal {
            self.status.say("portal disabled");
            return Ok(Portal(None));
        }
        Ok(Portal(Some(self.open_portal(assets)?)))
    }

    fn open_portal(&self, assets: &'static [FrontendAsset]) -> Result<WebPortal> {
        let options = &self.options;
        let access = options.access();
        let listener = TcpListener::bind(options.listen).map_err(|error| {
            Error::adapter(format!("cannot listen on {}", options.listen), error)
        })?;
        let address = listener
            .local_addr()
            .map_err(|error| Error::adapter("cannot read the portal address", error))?;
        let portal =
            WebPortal::from_listener(listener, assets, access.clone())?.metrics(options.metrics);
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

/// The host's web portal, or nothing under `--no-portal`.
pub struct Portal(Option<WebPortal>);

impl Portal {
    /// Also serve the capture stream a capture-only TV streams to, handing
    /// its connections to the remote source (`remote.incoming()`).
    #[must_use]
    pub fn capture_stream(self, incoming: Handoff) -> Self {
        Self(self.0.map(|portal| portal.capture_stream(incoming)))
    }
}

impl Surface for Portal {
    fn run(
        self: Box<Self>,
        engine: EngineHandle,
        shutdown: Shutdown,
    ) -> BoxFuture<'static, Result<()>> {
        match self.0 {
            Some(portal) => Box::new(portal).run(engine, shutdown),
            None => Box::pin(async { Ok(()) }),
        }
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

/// The status surface; see [`Host::status`].
#[derive(Debug, Clone)]
pub struct Status {
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

impl Surface for Status {
    fn run(
        self: Box<Self>,
        engine: EngineHandle,
        shutdown: Shutdown,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(self.watch(engine, shutdown))
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
        if let SessionState::Faulted(phase) = snapshot.session {
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
        let failure = snapshot
            .session
            .failure()
            .map(|failure| format!("{} ({})", chain(failure), failure.escalation().next_step()));
        if failure != self.failure {
            if let Some(failure) = &failure {
                lines.push(format!("lighting failed: {failure}"));
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
        // A stderr that is not the terminal is always written.
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
            // Taken: neither "target off" nor the way to running was printed.
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
            // Released as a full-screen surface stops: the final report.
            terminal.send_replace(false);
            assert_eq!(lines.recv().await.unwrap(), "target off, session stopped");
            Ok(())
        };
        let run = Prysm::builder(MemoryStore::with(fakes::settings()))
            .source(FakeSources(log.clone()))
            .output(FakeOutputs(log))
            .surface(status)
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
