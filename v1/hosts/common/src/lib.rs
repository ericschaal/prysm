//! Process concerns shared by the desktop hosts, so each host `main` is only
//! its builder list: arguments, logging, the portal listener, signals and a
//! status surface that reports to stderr.
//!
//! ```ignore
//! const PROGRAM: Program = Program { name: "prysm-headless", about: ABOUT, full_screen: false };
//!
//! fn main() -> ExitCode {
//!     prysm_host::main(PROGRAM, |host| async move {
//!         Prysm::builder(FileSettings::new(host.settings()))
//!             .output(Wled)
//!             .surface(host.status())
//!             .surface(host.portal(ASSETS)?)
//!             .run(host.signals()?)
//!             .await
//!     })
//! }
//! ```
mod args;

pub use args::{Options, Parsed, PortalOptions, parse};

use prysm_ports::{Error, LightingTarget, Result, SessionState, Snapshot, chain};
use prysm_runtime::logging::{self, Logs};
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use prysm_web_portal::{Access, FrontendAsset, WebPortal};
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{self, IsTerminal};
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::path::Path;
use std::process::ExitCode;

/// What a host executable is.
#[derive(Debug, Clone, Copy)]
pub struct Program {
    /// Prefixes every status and log line.
    pub name: &'static str,
    pub about: &'static str,
    /// A surface draws the whole terminal: while stderr is that terminal,
    /// status lines are not printed and log lines go only to `--log-file`.
    pub full_screen: bool,
}

/// Parse arguments, install logging, run `host` on a current-thread runtime
/// and map its result to an exit code: 0 after a clean stop, 1 if it failed,
/// 2 for bad arguments.
pub fn main<F, Fut>(program: Program, host: F) -> ExitCode
where
    F: FnOnce(Host) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let Program {
        name,
        about,
        full_screen,
    } = program;
    let options = match parse(std::env::args_os().skip(1)) {
        Ok(Parsed::Help) => {
            println!("{}", args::usage(name, about));
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Run(options)) => options,
        Err(message) => {
            eprintln!("{name}: {message}\n\n{}", args::usage(name, about));
            return ExitCode::from(2);
        }
    };
    let quiet = full_screen && io::stderr().is_terminal();
    let logs = match &options.log_file {
        Some(path) => match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => Some(Logs::File(file)),
            Err(error) => {
                eprintln!("{name}: cannot open log file {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None if quiet => None,
        None => Some(Logs::Stderr),
    };
    if let Some(logs) = logs {
        logging::init(name, logs);
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
    eprintln!("{name}: settings {}", options.settings.display());
    match runtime.block_on(host(Host {
        name,
        options,
        quiet,
    })) {
        Ok(()) => {
            eprintln!("{name}: stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{name}: {}", chain(&error));
            ExitCode::FAILURE
        }
    }
}

/// The parsed process options of a running host.
#[derive(Debug)]
pub struct Host {
    name: &'static str,
    options: Options,
    /// A full-screen surface owns stderr; see [`Program::full_screen`].
    quiet: bool,
}

impl Host {
    /// The JSON settings file.
    #[must_use]
    pub fn settings(&self) -> &Path {
        &self.options.settings
    }

    /// Resolves on SIGINT or SIGTERM (Ctrl-C elsewhere). Registered now, so a
    /// signal that arrives while the host starts is not lost.
    ///
    /// # Errors
    /// The signal handlers cannot be installed.
    pub fn signals(&self) -> Result<impl Future<Output = ()> + Send + 'static> {
        let name = self.name;
        let failed = |error| Error::adapter("cannot install signal handlers", error);
        #[cfg(unix)]
        let signal = {
            use tokio::signal::unix::{SignalKind, signal};
            let mut interrupt = signal(SignalKind::interrupt()).map_err(failed)?;
            let mut terminate = signal(SignalKind::terminate()).map_err(failed)?;
            async move {
                tokio::select! {
                    _ = interrupt.recv() => "SIGINT",
                    _ = terminate.recv() => "SIGTERM",
                }
            }
        };
        #[cfg(not(unix))]
        let signal = {
            let _ = failed;
            async {
                let _ = tokio::signal::ctrl_c().await;
                "Ctrl-C"
            }
        };
        Ok(async move {
            let signal = signal.await;
            eprintln!("{name}: {signal}, shutting down");
        })
    }

    /// Reports state changes and surface failures to stderr, unless a
    /// full-screen surface owns it, and requests processed lighting once if
    /// `--start` was given.
    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            name: self.name,
            start: self.options.start,
            print: !self.quiet,
        }
    }

    /// The web portal on its bound listener, or a surface that does nothing
    /// under `--no-portal`. Binds now, so the actual address is printed.
    ///
    /// # Errors
    /// Bind failure, or an origin `WebPortal` rejects.
    pub fn portal(&self, assets: &'static [FrontendAsset]) -> Result<impl Surface + use<>> {
        let portal = match &self.options.portal {
            None => {
                eprintln!("{}: portal disabled", self.name);
                None
            }
            Some(options) => Some(self.open_portal(options, assets)?),
        };
        Ok(move |engine: EngineHandle, shutdown: Shutdown| async move {
            match portal {
                Some(portal) => Box::new(portal).run(engine, shutdown).await,
                None => Ok(()),
            }
        })
    }

    fn open_portal(
        &self,
        options: &PortalOptions,
        assets: &'static [FrontendAsset],
    ) -> Result<WebPortal> {
        let listener = TcpListener::bind(options.listen).map_err(|error| {
            Error::adapter(format!("cannot listen on {}", options.listen), error)
        })?;
        let address = listener
            .local_addr()
            .map_err(|error| Error::adapter("cannot read the portal address", error))?;
        let portal = WebPortal::from_listener(listener, assets, options.access.clone())?;
        eprintln!(
            "{}: {} ({})",
            self.name,
            reachable(address),
            access(&options.access)
        );
        Ok(portal)
    }
}

/// Where the portal can be opened. On every interface, the address of the
/// interface with the default route, if there is one.
fn reachable(address: SocketAddr) -> String {
    let port = address.port();
    if !address.ip().is_unspecified() {
        return format!("portal http://{address}/");
    }
    match lan_address(address.ip()) {
        Some(ip) => format!(
            "portal http://{}/ on all interfaces",
            SocketAddr::new(ip, port)
        ),
        None => format!("portal on all interfaces, port {port}"),
    }
}

/// Connecting a UDP socket only picks a route and a local address; nothing is sent.
fn lan_address(unspecified: IpAddr) -> Option<IpAddr> {
    let (local, probe) = if unspecified.is_ipv4() {
        ("0.0.0.0:0", "192.0.2.1:9")
    } else {
        ("[::]:0", "[2001:db8::1]:9")
    };
    let socket = UdpSocket::bind(local).ok()?;
    socket.connect(probe).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

fn access(access: &Access) -> String {
    match access {
        Access::SameOrigin => "no password; other sites' pages refused".into(),
        Access::Origins(origins) => format!(
            "no password; other sites' pages refused except {}",
            origins.join(", ")
        ),
        Access::AnyOrigin => "no password; pages from any site allowed".into(),
    }
}

/// The status surface; see [`Host::status`].
#[derive(Debug)]
pub struct Status {
    name: &'static str,
    start: bool,
    print: bool,
}

impl Status {
    fn say(&self, line: &str) {
        if self.print {
            eprintln!("{}: {line}", self.name);
        }
    }

    async fn watch(self, engine: EngineHandle, shutdown: Shutdown) -> Result<()> {
        let mut snapshots = engine.snapshot();
        let mut seen = Seen::default();
        let mut start = self.start;
        loop {
            for line in seen.update(&snapshots.borrow_and_update()) {
                self.say(&line);
            }
            if std::mem::take(&mut start) {
                tokio::select! {
                    () = shutdown.requested() => return Ok(()),
                    started = engine.set_target(LightingTarget::Processed) => {
                        if let Err(error) = started {
                            self.say(&format!("--start failed: {}", chain(&error)));
                        }
                    }
                }
                continue;
            }
            tokio::select! {
                () = shutdown.requested() => return Ok(()),
                changed = snapshots.changed() => if changed.is_err() { return Ok(()) },
            }
        }
    }
}

impl Surface for Status {
    fn run(
        self: Box<Self>,
        engine: EngineHandle,
        shutdown: Shutdown,
    ) -> futures::future::BoxFuture<'static, Result<()>> {
        Box::pin(self.watch(engine, shutdown))
    }
}

/// What has already been reported, so only changes are printed.
#[derive(Debug, Default)]
struct Seen {
    state: Option<String>,
    failure: Option<String>,
    surface_failures: usize,
}

impl Seen {
    fn update(&mut self, snapshot: &Snapshot) -> Vec<String> {
        let mut lines = Vec::new();
        let state = format!(
            "target {}, session {}",
            target(snapshot.target),
            session(snapshot.session)
        );
        if self.state.as_ref() != Some(&state) {
            lines.push(state.clone());
            self.state = Some(state);
        }
        let failure = snapshot.failure.as_ref().map(|failure| chain(failure));
        if failure != self.failure {
            if let Some(failure) = &failure {
                lines.push(format!("lighting failed: {failure}"));
            }
            self.failure = failure;
        }
        for failure in snapshot.surface_failures.iter().skip(self.surface_failures) {
            lines.push(format!(
                "surface failed, lighting continues: {}",
                chain(failure)
            ));
        }
        self.surface_failures = snapshot.surface_failures.len();
        lines
    }
}

fn target(target: LightingTarget) -> &'static str {
    match target {
        LightingTarget::Off => "off",
        LightingTarget::Processed => "processed",
        LightingTarget::DeviceNative => "device native",
    }
}

fn session(state: SessionState) -> String {
    match state {
        SessionState::Stopped => "stopped".into(),
        SessionState::Running => "running".into(),
        SessionState::Native => "native".into(),
        SessionState::Faulted(phase) => format!("faulted while {phase}"),
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

        snapshot.target = LightingTarget::Processed;
        snapshot.failure = Some(Error::other("no frames"));
        snapshot.surface_failures.push(Error::other("no terminal"));
        assert_eq!(
            seen.update(&snapshot),
            [
                "target processed, session stopped",
                "lighting failed: no frames",
                "surface failed, lighting continues: no terminal",
            ]
        );
        assert!(seen.update(&snapshot).is_empty());

        snapshot.session = SessionState::Faulted(prysm_ports::SessionPhase::IdlingOutput);
        snapshot.failure = None;
        assert_eq!(
            seen.update(&snapshot),
            ["target processed, session faulted while idling the output"]
        );
    }
}
