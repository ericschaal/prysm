//! WLED as a blocking [`prysm_ports::LedOutput`].
//!
//! - [`Wled`] opens an output: it resolves the configured host (a name such as
//!   `wled.local` or an IP literal, with an optional HTTP port, default 80) and
//!   switches the device to live mode over the JSON API (`POST /json/state`).
//!   The resolved IP is used for HTTP and DDP until the output is dropped; the
//!   next open resolves again.
//! - `write` sends colours as DDP over UDP (port 4048), clockwise from the
//!   bottom-left corner.
//! - `idle` powers the device off or returns it to its own effect; a failed open
//!   turns it off before returning.
//!
//! No threads or timers beyond a bounded resolver thread at open: the runtime rewrites the last colours while the source
//! is quiet, which keeps WLED in live mode.
mod address;
mod http;
mod output;
mod settings;

pub use output::Wled;
pub use settings::{LedCounts, WledSettings};

#[cfg(test)]
mod tests;
