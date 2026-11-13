//! Process concerns shared by the desktop hosts, so each host `main` is only
//! its builder list: arguments, logging, the portal listener, signals, a
//! status surface that reports to stderr and the `--start` request.
//!
//! ```ignore
//! const PROGRAM: Program = Program { name: "prysm-headless", about: ABOUT, full_screen: false };
//!
//! fn main() -> ExitCode {
//!     prysm_host::main(PROGRAM, adapters, |host, prysm| async move {
//!         prysm
//!             .surface(host.status())
//!             .surface(host.start())
//!             .surface(host.portal(ASSETS)?)
//!             .run(host.signals()?)
//!             .await
//!     })
//! }
//!
//! fn adapters(prysm: Builder) -> Builder {
//!     prysm.source(DeviceSource).output(Wled::default())
//! }
//! ```
//!
//! `--check-settings` is handled here: the settings file is validated against
//! the host's adapters and the run closure is never called.
mod args;

pub use args::{Options, parse};

use futures::future::BoxFuture;
use prysm_ports::{
    Error, Handoff, LightingTarget, OutputIssue, Result, SessionState, Snapshot, chain,
};
use prysm_runtime::logging::{self, Logs};
use prysm_runtime::{Builder, EngineHandle, Prysm, Shutdown, Surface};
use prysm_settings_file::SettingsFile;
use prysm_web_portal::{Access, FrontendAsset, WebPortal, lan_url};
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{self, IsTerminal};
use std::net::{SocketAddr, TcpListener};
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

/// Parse arguments, install logging, register `adapters` on a builder over
/// the settings file, run `host` with it on a current-thread runtime and map
/// its result to an exit code: 0 after a clean stop or `--help`/`--version`,
/// 1 if it failed, 2 for bad arguments. Under `--check-settings` only validates the settings
/// ([`Builder::check`]): 0 if valid, else 1; `host` is not called.
pub fn main<A, F, Fut>(program: Program, adapters: A, host: F) -> ExitCode
where
    A: FnOnce(Builder) -> Builder,
    F: FnOnce(Host, Builder) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let Program {
        name,
        about,
        full_screen,
    } = program;
    let options = parse(name, about, std::env::args_os()).unwrap_or_else(|error| error.exit());
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
    eprintln!("{name}: settings {}", options.settings.display());
    let prysm = adapters(Prysm::builder(SettingsFile::new(&options.settings)));
    if options.check_settings {
        return match prysm.check() {
            Ok(()) => {
                eprintln!("{name}: settings OK");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{name}: {}", chain(&error));
                ExitCode::FAILURE
            }
        };
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
    match runtime.block_on(host(
        Host {
            name,
            options,
            quiet,
        },
        prysm,
    )) {
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
    /// full-screen surface owns it.
    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            name: self.name,
            print: !self.quiet,
        }
    }

    /// Requests processed lighting once if `--start` was given, then ends.
    /// A failure is printed like a status line; the snapshot keeps it too.
    #[must_use]
    pub fn start(&self) -> impl Surface + use<> {
        let start = self.options.start;
        let status = self.status();
        move |engine: EngineHandle, shutdown: Shutdown| async move {
            if start {
                tokio::select! {
                    () = shutdown.requested() => {}
                    started = engine.set_target(LightingTarget::Processed) => {
                        if let Err(error) = started {
                            status.say(&format!("--start failed: {}", chain(&error)));
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
            eprintln!("{}: portal disabled", self.name);
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
        eprintln!(
            "{}: {} ({}{})",
            self.name,
            reachable(address),
            describe(&access),
            if options.metrics {
                "; metrics at /metrics"
            } else {
                ""
            }
        );
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

/// The status surface; see [`Host::status`].
#[derive(Debug)]
pub struct Status {
    name: &'static str,
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
        loop {
            for line in seen.update(&snapshots.borrow_and_update()) {
                self.say(&line);
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
    output_issue: Option<OutputIssue>,
    surface_failures: usize,
}

impl Seen {
    fn update(&mut self, snapshot: &Snapshot) -> Vec<String> {
        let mut lines = Vec::new();
        let state = format!(
            "target {}, session {}",
            snapshot.target.as_str(),
            snapshot.session
        );
        if self.state.as_ref() != Some(&state) {
            lines.push(state.clone());
            self.state = Some(state);
        }
        if snapshot.output_issue != self.output_issue {
            match snapshot.output_issue {
                Some(issue) => lines.push(format!(
                    "lights not showing Prysm, lighting continues: {issue}"
                )),
                None if snapshot.session == SessionState::Running => {
                    lines.push("lights showing Prysm again".to_owned());
                }
                None => {}
            }
            self.output_issue = snapshot.output_issue;
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

        snapshot.session = SessionState::Running;
        snapshot.failure = None;
        let _ = seen.update(&snapshot);
        snapshot.output_issue = Some(OutputIssue::Off);
        assert_eq!(
            seen.update(&snapshot),
            ["lights not showing Prysm, lighting continues: switched off on the device"]
        );
        snapshot.output_issue = None;
        assert_eq!(seen.update(&snapshot), ["lights showing Prysm again"]);

        snapshot.session = SessionState::Faulted(prysm_ports::SessionPhase::IdlingOutput);
        snapshot.failure = None;
        assert_eq!(
            seen.update(&snapshot),
            ["target processed, session faulted while idling the output"]
        );
    }
}
