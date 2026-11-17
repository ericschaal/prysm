//! The TV supervisor: owns the Luna name, TV power and video, the TV-local
//! preferences and the one worker process; installs and removes the
//! package's registration. Design: `docs/plans/prysm-v1-webos-supervisor.md`.
//!
//! - [`machine`]: every decision, as a pure state machine.
//! - [`service`]: the driver, behind the [`service::Bus`],
//!   [`launcher::Launcher`], [`preferences::PreferencesStore`] and
//!   [`install::Installer`] seams.
//! - [`main`]: the process: arguments, signals, the real Luna bus.
//! - [`lock`]: one supervisor per identity, stopped by the `uninstall`
//!   subcommand before it deregisters.
//! - `schema` (feature `schema`): the `luna` OpenAPI document of the TV app's API.
mod cli;
pub mod identity;
pub mod install;
pub mod launcher;
pub mod lock;
mod luna;
pub mod machine;
pub mod observe;
pub mod preferences;
#[cfg(feature = "schema")]
pub mod schema;
pub mod service;
pub mod state;

pub use cli::main;
