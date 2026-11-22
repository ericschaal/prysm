//! WLED as a blocking [`prysm_ports::LedOutput`].
//!
//! - [`Wled`] opens an output: it resolves the configured host (a name such as
//!   `wled.local` or an IP literal, with an optional HTTP port, default 80) and
//!   switches the device to live mode over the JSON API (`POST /json/state`).
//!   The resolved IP is used for HTTP and DDP until the output is dropped; the
//!   next open resolves again.
//! - `write` sends colours as DDP over UDP (port 4048), clockwise from the
//!   bottom-left corner, encoded for the gamma WLED applies (read with its
//!   configuration; sRGB until then) and, unless `dither` is off, dithered
//!   over the levels WLED shows at its brightness. A frame the network
//!   refuses is dropped; `write` fails once sends have kept failing for 3 s.
//! - `idle` powers the device off or returns it to its own effect; a failed open
//!   turns it off before returning. [`Wled`]'s own `idle` does the same for a
//!   device nobody holds, without live mode first. Either takes at most
//!   [`IDLE_LIMIT`], name resolution included.
//! - `report` checks WLED's own state about once a second without waiting for
//!   the connect or the reply, and reports when it is not showing the colours:
//!   switched off, live override on, not in live mode, another sender, or no
//!   answer; with WLED's brightness and LED count. Prysm does not switch WLED
//!   back; the colours show again once WLED takes them.
//!
//! - `candidates` finds devices on the local network for the portal's setup:
//!   one multicast DNS question for `_wled._tcp.local`, then `GET /json/info`
//!   per device for its name and LED count (see `discover`).
//!
//! No background threads: a host name's lookup runs on a helper thread that
//! open and idle wait for within a deadline (a lookup still running is kept
//! for the next one), and discovery's requests run on scoped threads it
//! joins. The time limits are deadlines on calls, never retries:
//! `http::TIMEOUT` for a resolution or a mode change,
//! the state check's cadence, backoff and reply deadline, `ddp`'s
//! `SEND_GRACE` before failing sends fail `write`, and `discover`'s answer
//! window and per-device request limit. The runtime rewrites the last colours
//! while the source is briefly quiet, which keeps WLED in live mode, and after
//! a few seconds without frames idles the output `Native` (WLED's own state)
//! and opens it again at the next frame.
mod address;
mod ddp;
mod discover;
mod dither;
mod http;
mod observe;
mod output;
mod settings;

pub use output::{IDLE_LIMIT, OPEN_LIMIT, Wled};
pub use settings::{LedCounts, MAX_LEDS, WledSettings};

#[cfg(test)]
mod tests;
