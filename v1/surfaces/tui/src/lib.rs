//! Terminal surface for the Prysm runtime: shows what the LEDs show, for local
//! testing without physical LEDs (pair it with [`prysm_runtime::NullOutput`]).
//!
//! The screen draws the captured frame as half blocks inside a perimeter of the
//! processed edge colors, with the engine snapshot (target, session, settings
//! revision, adapters) and diagnostics (lighting failure, surface failures and
//! command results). Keys:
//!
//! | Key | Effect |
//! | --- | --- |
//! | `p` | target processed |
//! | `n` | target device native |
//! | `o` | target off |
//! | `:` | command bar: `on`, `off`, `native`, `load <path>`, `quit`; Enter runs, Esc cancels |
//! | `q`, Esc, Ctrl-C, Ctrl-D | quit |
//!
//! `load <path>` replaces all settings with a JSON settings file at the
//! revision on screen. In raw mode Ctrl-C is a key, not a signal: pass
//! [`Tui::quit`] to the host's shutdown future so quitting stops the host.
//!
//! ```no_run
//! # async fn host(store: impl prysm_ports::SettingsStore + 'static) -> prysm_ports::Result<()> {
//! use prysm_runtime::{NullOutput, Prysm};
//! use prysm_surface_tui::Tui;
//!
//! let tui = Tui::new();
//! let quit = tui.quit();
//! Prysm::builder(store)
//!     .output(NullOutput)
//!     .surface(tui)
//!     .run(quit.requested())
//!     .await
//! # }
//! ```
mod command;
mod input;
mod screen;
mod video;
mod view;

use futures::FutureExt;
use futures::future::{BoxFuture, Fuse, FusedFuture};
use input::Notice;
use prysm_ports::{Error, Result};
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use screen::{Exit, Feed, ScreenThread};
use std::io::{self, IsTerminal};
use std::sync::mpsc as std_mpsc;
use tokio::sync::{mpsc, watch};

/// Commands waiting while one runs; more are refused with a "busy" notice.
const QUEUED_COMMANDS: usize = 4;

/// The terminal surface. Requires an interactive stdout; otherwise `run`
/// fails at once without writing to it, and lighting continues.
#[derive(Debug)]
pub struct Tui {
    quit: watch::Sender<bool>,
}

impl Default for Tui {
    fn default() -> Self {
        Self::new()
    }
}

impl Tui {
    #[must_use]
    pub fn new() -> Self {
        Self {
            quit: watch::channel(false).0,
        }
    }

    /// Resolves when the user quits the terminal.
    #[must_use]
    pub fn quit(&self) -> Quit {
        Quit(self.quit.subscribe())
    }

    async fn serve(self, engine: EngineHandle, shutdown: Shutdown) -> Result<()> {
        if !io::stdout().is_terminal() {
            return Err(Error::other(
                "the terminal surface requires stdout to be an interactive terminal",
            ));
        }
        let (commands, mut queued) = mpsc::channel(QUEUED_COMMANDS);
        let (notify, notices) = std_mpsc::channel();
        let mut screen = ScreenThread::spawn(Feed {
            snapshot: engine.snapshot(),
            preview: engine.preview(),
            commands,
            notices,
        })?;
        // The command being executed; terminated while none is.
        let mut running: Fuse<BoxFuture<'static, Notice>> = Fuse::terminated();
        let ended = loop {
            tokio::select! {
                () = shutdown.requested() => break None,
                ended = &mut screen.done => break Some(ended),
                Some(command) = queued.recv(), if running.is_terminated() => {
                    running = command::execute(engine.clone(), command).boxed().fuse();
                }
                notice = &mut running, if !running.is_terminated() => {
                    let _ = notify.send(notice);
                }
            }
        };
        // An unfinished command is dropped; the engine completes or discards it.
        drop(running);
        let ended = match ended {
            Some(ended) => ended,
            None => {
                screen.request_stop();
                (&mut screen.done).await
            }
        };
        match ended.map_err(|_| Error::Panicked("terminal surface"))?? {
            Exit::Quit => {
                self.quit.send_replace(true);
                Ok(())
            }
            Exit::Stopped => Ok(()),
        }
    }
}

impl Surface for Tui {
    fn run(
        self: Box<Self>,
        engine: EngineHandle,
        shutdown: Shutdown,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(self.serve(engine, shutdown))
    }
}

/// Resolves once the user quits the terminal surface. Never resolves if the
/// surface fails or ends any other way, so a host keeps lighting.
#[derive(Debug, Clone)]
pub struct Quit(watch::Receiver<bool>);

impl Quit {
    pub async fn requested(mut self) {
        if self.0.wait_for(|quit| *quit).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::OutputFactory;
    use prysm_runtime::fakes::{self, CallLog, FakeOutputs, FakeSources, MemoryStore};
    use prysm_runtime::{NullOutput, Prysm};
    use std::time::Duration;

    #[tokio::test]
    async fn without_a_terminal_the_surface_fails_and_lighting_continues() {
        if io::stdout().is_terminal() {
            return; // Only meaningful when tests run without a terminal.
        }
        let log = CallLog::new();
        let tui = Tui::new();
        let quit = tui.quit();
        let mut settings = fakes::settings();
        settings.output = Some(prysm_ports::AdapterSettings::new(
            NullOutput::KIND,
            &prysm_runtime::NullSettings::new(),
        ));
        let (report, reported) = tokio::sync::oneshot::channel();
        let run = Prysm::builder(MemoryStore::with(settings))
            .source(FakeSources(log.clone()))
            .output(FakeOutputs(log.clone()))
            .output(NullOutput)
            .surface(fakes::light_until_shutdown)
            .surface(tui)
            .surface(move |engine: EngineHandle, shutdown: Shutdown| async move {
                let mut snapshot = engine.snapshot();
                let failed = snapshot.wait_for(|s| !s.surface_failures.is_empty()).await;
                let message = failed.map_err(|_| Error::Closed)?.surface_failures[0].to_string();
                let _ = report.send(message);
                shutdown.requested().await;
                Ok(())
            })
            .run(async {
                // Lighting keeps running after the surface failure.
                assert!(log.wait_for("source.open:", 1).await);
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("host did not stop")
            .unwrap();
        let message = reported.await.unwrap();
        assert!(message.contains("interactive terminal"), "{message}");
        // The failed surface never reports a quit.
        let quit = tokio::time::timeout(Duration::from_millis(20), quit.requested()).await;
        assert!(quit.is_err());
    }

    #[tokio::test]
    async fn quit_resolves_only_on_request() {
        let tui = Tui::new();
        let pending = tui.quit();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), pending.requested())
                .await
                .is_err()
        );
        let quit = tui.quit();
        tui.quit.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), quit.requested())
            .await
            .unwrap();
        let dropped = Tui::new().quit();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), dropped.requested())
                .await
                .is_err()
        );
    }
}
