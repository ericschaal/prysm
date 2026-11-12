//! WLED as a blocking [`prysm_ports::LedOutput`].
//!
//! - [`Wled`] opens an output: it switches the device to live mode over the JSON
//!   API (`POST /json/state`, port 80).
//! - `write` sends colours as DDP over UDP (port 4048), clockwise from the
//!   bottom-left corner.
//! - `idle` powers the device off or returns it to its own effect; a failed open
//!   turns it off before returning.
//!
//! No threads or timers: the runtime rewrites the last colours while the source
//! is quiet, which keeps WLED in live mode.
mod http;
mod output;
mod settings;

pub use output::Wled;
pub use settings::{LedCounts, WledSettings};

#[cfg(test)]
mod tests;
