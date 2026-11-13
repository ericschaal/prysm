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
//! - `observe` checks WLED's own state about once a second without waiting for
//!   the reply, and reports when it is not showing the colours: switched off,
//!   live override on, not in live mode, another sender, or no answer. Prysm
//!   does not switch WLED back; the colours show again once WLED takes them.
//!
//! - `candidates` finds devices on the local network for the portal's setup:
//!   one multicast DNS question for `_wled._tcp.local`, then `GET /json/info`
//!   per device for its name and LED count (see `discover`).
//!
//! No threads or timers beyond a lookup thread for a host name (open waits for
//! it within a deadline; a lookup still running is kept for the next open): the runtime
//! rewrites the last colours while the source is briefly quiet, which keeps
//! WLED in live mode, and after a few seconds without frames idles the output
//! `Native` (WLED's own state) and opens it again at the next frame.
mod address;
mod discover;
mod http;
mod observe;
mod output;
mod settings;

pub use output::Wled;
pub use settings::{LedCounts, WledSettings};

#[cfg(test)]
mod tests;
